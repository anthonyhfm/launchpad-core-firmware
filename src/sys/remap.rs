// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister
// Remap mode storage and protocol, added for the Launchpad Mini mk1.

//! User-defined button remapping: a table of up to [`BANKS`] banks, each with
//! one [`Entry`] per surface index (0..[`SLOTS`]), stored in internal flash.
//!
//! Flash layout (STM32F103C8, 64 KB, 1 KB pages):
//!
//! ```text
//! 0x0800_0000  Novation bootloader (do not touch)
//! 0x0800_3000  CoreFW application (linker limit: 0x0800_F000, see memory.x)
//! 0x0800_F000  remap bank 0  (this module)
//! 0x0800_F400  remap bank 1
//! 0x0800_F800  remap bank 2
//! 0x0800_FC00  Novation bootloader settings page (do not touch)
//! ```
//!
//! Banks survive firmware updates: the bootloader only erases pages an update
//! covers, and the firmware stays below 0x0800_F000.
//!
//! SysEx protocol (all on the MIDI port, header `F0 7D 4C 52`, "LR"):
//!
//! ```text
//! 00                         info   -> F0 7D 4C 52 00 <proto> <banks> <slots> <active> <stored mask> F7
//! 01 <bank> <slot> <8 bytes> set entry in RAM
//! 02 <bank>                  commit bank to flash -> F0 7D 4C 52 02 <bank> <ok> F7
//! 03 <bank> <slot>           get entry -> F0 7D 4C 52 03 <bank> <slot> <8 bytes> F7
//! 04 <bank>                  clear bank in RAM (commit to persist)
//! 05 <bank>                  select active bank
//! 06                         reload all banks from flash (drop uncommitted edits)
//! ```

use core::cell::UnsafeCell;
use core::ptr;

use stm32_metapac as pac;

use crate::driver;
use crate::sys::midi::MidiPort;

pub const BANKS: usize = 3;
pub const SLOTS: usize = 100;
pub const ENTRY_BYTES: usize = 8;
pub const PROTOCOL_VERSION: u8 = 1;

pub const KIND_NONE: u8 = 0;
pub const KIND_NOTE: u8 = 1;
pub const KIND_CC: u8 = 2;
pub const KIND_PROGRAM: u8 = 3;
pub const KIND_BANK: u8 = 4;
pub const KIND_NATIVE: u8 = 5;
const KIND_MAX: u8 = KIND_NATIVE;

pub const FLAG_TOGGLE: u8 = 0x01;

const SYSEX_HEADER: [u8; 4] = [0xf0, 0x7d, 0x4c, 0x52];

const FLASH_PAGE_SIZE: usize = 1024;
const STORE_START: u32 = 0x0800_f000;
const STORE_END: u32 = STORE_START + (BANKS * FLASH_PAGE_SIZE) as u32;
// Never reach the bootloader settings page at 0x0800FC00.
const _: () = assert!(STORE_END <= 0x0800_fc00);
const _: () = assert!(16 + SLOTS * ENTRY_BYTES <= FLASH_PAGE_SIZE);

const PAGE_MAGIC: [u8; 4] = *b"LPRM";
const PAGE_FORMAT: u8 = 1;
const PAGE_HEADER: usize = 16;

const FLASH_KEY1: u32 = 0x4567_0123;
const FLASH_KEY2: u32 = 0xcdef_89ab;

/// One button mapping. All fields are 7-bit to travel in SysEx unchanged.
///
/// `color_on` / `color_off`: bits 0-1 red level, bits 2-3 green level (0..3),
/// matching the mk1's red/green LEDs.
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct Entry {
    pub kind: u8,
    pub flags: u8,
    pub channel: u8,
    pub number: u8,
    pub on_value: u8,
    pub off_value: u8,
    pub color_on: u8,
    pub color_off: u8,
}

impl Entry {
    pub const NONE: Self = Self {
        kind: KIND_NONE,
        flags: 0,
        channel: 0,
        number: 0,
        on_value: 0,
        off_value: 0,
        color_on: 0,
        color_off: 0,
    };

    fn from_bytes(b: &[u8]) -> Self {
        let kind = if b[0] <= KIND_MAX { b[0] } else { KIND_NONE };
        Self {
            kind,
            flags: b[1] & FLAG_TOGGLE,
            channel: b[2] & 0x0f,
            number: b[3] & 0x7f,
            on_value: b[4] & 0x7f,
            off_value: b[5] & 0x7f,
            color_on: b[6] & 0x0f,
            color_off: b[7] & 0x0f,
        }
    }

    fn to_bytes(self) -> [u8; ENTRY_BYTES] {
        [
            self.kind,
            self.flags,
            self.channel,
            self.number,
            self.on_value,
            self.off_value,
            self.color_on,
            self.color_off,
        ]
    }

    pub fn is_toggle(&self) -> bool {
        self.flags & FLAG_TOGGLE != 0
    }
}

/// `staging_bank` is 0 when nothing is staged, otherwise 1 + bank. Zero init
/// keeps the state in .bss (no flash cost).
const NO_BANK: u8 = 0;

/// Runtime state. Stored banks are read straight from (memory-mapped) flash;
/// only the bank currently being edited over SysEx lives in RAM.
pub struct State {
    loaded: bool,
    /// Flash page holds a valid bank (magic, format, CRC).
    valid: [bool; BANKS],
    /// Valid and contains at least one mapped entry.
    stored: [bool; BANKS],
    staging: [Entry; SLOTS],
    staging_bank: u8,
    pub active: u8,
    /// Lit / toggled bit per bank and slot (LED shows color_on when set).
    lit: [[u32; 4]; BANKS],
    /// Bumped whenever the table or active bank changes; apps redraw on change.
    pub generation: u16,
}

impl State {
    const fn new() -> Self {
        Self {
            loaded: false,
            valid: [false; BANKS],
            stored: [false; BANKS],
            staging: [Entry::NONE; SLOTS],
            staging_bank: NO_BANK,
            active: 0,
            lit: [[0; 4]; BANKS],
            generation: 0,
        }
    }

    fn ensure_loaded(&mut self) {
        if !self.loaded {
            self.reload();
        }
    }

    #[inline(never)]
    fn reload(&mut self) {
        for bank in 0..BANKS {
            self.valid[bank] = flash_bank_valid(bank);
            self.stored[bank] =
                self.valid[bank] && (0..SLOTS).any(|slot| flash_entry(bank, slot).kind != KIND_NONE);
        }
        self.staging_bank = NO_BANK;
        self.lit = [[0; 4]; BANKS];
        self.loaded = true;
        self.bump();
    }

    fn bump(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn entry(&self, bank: u8, slot: u8) -> Entry {
        let (b, s) = (bank as usize, slot as usize);
        if b >= BANKS || s >= SLOTS {
            Entry::NONE
        } else if bank + 1 == self.staging_bank {
            self.staging[s]
        } else if self.valid[b] {
            flash_entry(b, s)
        } else {
            Entry::NONE
        }
    }

    pub fn is_lit(&self, bank: u8, slot: u8) -> bool {
        let (b, s) = (bank as usize, slot as usize);
        b < BANKS && s < SLOTS && self.lit[b][s / 32] & (1 << (s % 32)) != 0
    }

    pub fn set_lit(&mut self, bank: u8, slot: u8, on: bool) {
        let (b, s) = (bank as usize, slot as usize);
        if b < BANKS && s < SLOTS {
            if on {
                self.lit[b][s / 32] |= 1 << (s % 32);
            } else {
                self.lit[b][s / 32] &= !(1 << (s % 32));
            }
        }
    }

    pub fn select_bank(&mut self, bank: u8) {
        if (bank as usize) < BANKS && bank != self.active {
            self.active = bank;
            self.bump();
        }
    }

    /// Make `bank` the staging bank, seeding it from its current contents.
    fn stage(&mut self, bank: u8) {
        if self.staging_bank != bank + 1 {
            for slot in 0..SLOTS {
                self.staging[slot] = if self.valid[bank as usize] {
                    flash_entry(bank as usize, slot)
                } else {
                    Entry::NONE
                };
            }
            self.staging_bank = bank + 1;
        }
    }

    fn commit(&mut self, bank: u8) -> bool {
        let b = bank as usize;
        if self.staging_bank != bank + 1 {
            // Nothing edited: report whether flash already holds a valid bank.
            return self.valid[b];
        }
        let ok = write_bank(b, &self.staging);
        self.valid[b] = flash_bank_valid(b);
        if ok && self.valid[b] {
            self.stored[b] = self.staging.iter().any(|e| e.kind != KIND_NONE);
            self.staging_bank = NO_BANK;
        }
        self.bump();
        ok && self.valid[b]
    }

    fn stored_mask(&self) -> u8 {
        let mut mask = 0;
        for bank in 0..BANKS {
            if self.stored[bank] {
                mask |= 1 << bank;
            }
        }
        mask
    }
}

struct StateSlot(UnsafeCell<State>);

// Only touched from the main loop (app callbacks and SysEx dispatch), never
// from interrupt handlers.
unsafe impl Sync for StateSlot {}

static STATE: StateSlot = StateSlot(UnsafeCell::new(State::new()));

/// Run `f` with the remap state, validating flash banks on first use.
pub fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    let state = unsafe { &mut *STATE.0.get() };
    state.ensure_loaded();
    f(state)
}

/// True when at least one bank holds a stored mapping (used to pick the
/// start-up mode).
pub fn has_stored_table() -> bool {
    with(|s| s.stored_mask() != 0)
}

// ---------------------------------------------------------------------------
// SysEx

#[inline(never)]
pub fn execute(port: MidiPort, data: &[u8]) -> bool {
    if data.len() < 6 || data[..4] != SYSEX_HEADER || data[data.len() - 1] != 0xf7 {
        return false;
    }
    let body = &data[4..data.len() - 1];
    let cmd = body[0];
    let args = &body[1..];

    match cmd {
        0x00 => {
            let (active, mask) = with(|s| (s.active, s.stored_mask()));
            reply(
                port,
                &[
                    0x00,
                    PROTOCOL_VERSION,
                    BANKS as u8,
                    SLOTS as u8,
                    active,
                    mask,
                ],
            );
        }
        0x01 if args.len() >= 2 + ENTRY_BYTES => {
            let (bank, slot) = (args[0], args[1]);
            if (bank as usize) < BANKS && (slot as usize) < SLOTS {
                let entry = Entry::from_bytes(&args[2..2 + ENTRY_BYTES]);
                with(|s| {
                    s.stage(bank);
                    s.staging[slot as usize] = entry;
                    s.set_lit(bank, slot, false);
                    s.bump();
                });
            }
        }
        0x02 if !args.is_empty() => {
            let bank = args[0];
            let ok = (bank as usize) < BANKS && with(|s| s.commit(bank));
            reply(port, &[0x02, bank, ok as u8]);
        }
        0x03 if args.len() >= 2 => {
            let (bank, slot) = (args[0], args[1]);
            let bytes = with(|s| s.entry(bank, slot).to_bytes());
            let mut out = [0u8; 3 + ENTRY_BYTES];
            out[0] = 0x03;
            out[1] = bank;
            out[2] = slot;
            out[3..].copy_from_slice(&bytes);
            reply(port, &out);
        }
        0x04 if !args.is_empty() => {
            let bank = args[0];
            if (bank as usize) < BANKS {
                with(|s| {
                    s.stage(bank);
                    s.staging = [Entry::NONE; SLOTS];
                    s.lit[bank as usize] = [0; 4];
                    s.bump();
                });
            }
        }
        0x05 if !args.is_empty() => with(|s| s.select_bank(args[0])),
        0x06 => with(|s| s.reload()),
        _ => {}
    }
    true
}

fn reply(port: MidiPort, payload: &[u8]) {
    let mut out = [0u8; 4 + 16 + 1];
    let len = 4 + payload.len().min(16);
    out[..4].copy_from_slice(&SYSEX_HEADER);
    out[4..len].copy_from_slice(&payload[..len - 4]);
    out[len] = 0xf7;
    driver::send_midi(port, &out[..len + 1]);
}

// ---------------------------------------------------------------------------
// Flash

fn page_address(bank: usize) -> u32 {
    STORE_START + (bank * FLASH_PAGE_SIZE) as u32
}

fn crc32(bytes: impl Iterator<Item = u8>) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn flash_byte(address: u32) -> u8 {
    unsafe { ptr::read_volatile(address as *const u8) }
}

fn flash_entry(bank: usize, slot: usize) -> Entry {
    let base = page_address(bank) + (PAGE_HEADER + slot * ENTRY_BYTES) as u32;
    let mut bytes = [0u8; ENTRY_BYTES];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = flash_byte(base + i as u32);
    }
    Entry::from_bytes(&bytes)
}

#[inline(never)]
fn flash_bank_valid(bank: usize) -> bool {
    let base = page_address(bank);
    let header: [u8; 12] = core::array::from_fn(|i| flash_byte(base + i as u32));
    if header[..4] != PAGE_MAGIC || header[4] != PAGE_FORMAT || header[5] as usize != bank {
        return false;
    }
    let crc = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let payload = (0..SLOTS * ENTRY_BYTES).map(|i| flash_byte(base + (PAGE_HEADER + i) as u32));
    crc == crc32(payload)
}

/// Byte `i` of the page image for `entries` (header, entries, then erased).
fn page_byte(bank: usize, entries: &[Entry; SLOTS], crc: u32, i: usize) -> u8 {
    match i {
        0..=3 => PAGE_MAGIC[i],
        4 => PAGE_FORMAT,
        5 => bank as u8,
        6 | 7 => 0,
        8..=11 => crc.to_le_bytes()[i - 8],
        12..=15 => 0xff,
        _ if i < PAGE_HEADER + SLOTS * ENTRY_BYTES => {
            let rel = i - PAGE_HEADER;
            entries[rel / ENTRY_BYTES].to_bytes()[rel % ENTRY_BYTES]
        }
        _ => 0xff,
    }
}

#[inline(never)]
fn write_bank(bank: usize, entries: &[Entry; SLOTS]) -> bool {
    let crc = crc32(entries.iter().flat_map(|e| e.to_bytes()));
    let address = page_address(bank);
    unlock();
    let ok = erase_page(address)
        && program_page(address, |i| page_byte(bank, entries, crc, i));
    lock();
    ok && (0..FLASH_PAGE_SIZE)
        .all(|i| flash_byte(address + i as u32) == page_byte(bank, entries, crc, i))
}

fn in_store(address: u32, len: usize) -> bool {
    address >= STORE_START && address + len as u32 <= STORE_END && address % 2 == 0
}

fn unlock() {
    if pac::FLASH.cr().read().lock() {
        pac::FLASH.keyr().write_value(FLASH_KEY1);
        pac::FLASH.keyr().write_value(FLASH_KEY2);
    }
}

fn lock() {
    pac::FLASH.cr().modify(|w| w.set_lock(true));
}

fn wait_ready() -> bool {
    while pac::FLASH.sr().read().bsy() {}
    let status = pac::FLASH.sr().read();
    let failed = status.pgerr() || status.wrprterr();
    pac::FLASH.sr().write(|w| {
        w.set_eop(true);
        w.set_pgerr(true);
        w.set_wrprterr(true);
    });
    !failed
}

fn erase_page(address: u32) -> bool {
    if !in_store(address, FLASH_PAGE_SIZE) || address % FLASH_PAGE_SIZE as u32 != 0 {
        return false;
    }
    if !wait_ready() {
        return false;
    }
    pac::FLASH.cr().write(|w| w.set_per(true));
    pac::FLASH.ar().write(|w| w.set_far(address));
    pac::FLASH.cr().modify(|w| w.set_strt(true));
    let ok = wait_ready();
    pac::FLASH.cr().write(|_| {});
    ok
}

fn program_page(address: u32, byte_at: impl Fn(usize) -> u8) -> bool {
    if !in_store(address, FLASH_PAGE_SIZE) || !wait_ready() {
        return false;
    }
    pac::FLASH.cr().write(|w| w.set_pg(true));
    let mut ok = true;
    for index in 0..FLASH_PAGE_SIZE / 2 {
        let halfword = u16::from_le_bytes([byte_at(index * 2), byte_at(index * 2 + 1)]);
        if halfword == 0xffff {
            continue;
        }
        let target = address + (index * 2) as u32;
        if !in_store(target, 2) {
            ok = false;
            break;
        }
        unsafe { ptr::write_volatile(target as *mut u16, halfword) };
        if !wait_ready() {
            ok = false;
            break;
        }
    }
    pac::FLASH.cr().write(|_| {});
    ok
}
