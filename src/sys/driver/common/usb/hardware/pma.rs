// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use super::super::UsbDeviceConfig;
use super::super::control::{SetupAction, control, handle_setup_request, next_ep0_chunk};
use super::super::descriptors::UsbMode;
use super::super::dmx::{DmxParser, dmx_queues};
use super::super::midi::{SysexReceiver, parse_usb_midi_packet};
use super::super::queues::queues;
use core::cell::UnsafeCell;
use core::ptr;
use stm32_metapac as pac;

const USB_BASE: usize = 0x4000_5c00;
const USB_PMA: usize = 0x4000_6000;

const USB_CNTR: *mut u32 = (USB_BASE + 0x40) as *mut u32;
const USB_ISTR: *mut u32 = (USB_BASE + 0x44) as *mut u32;
const USB_DADDR: *mut u32 = (USB_BASE + 0x4c) as *mut u32;
const USB_BTABLE: *mut u32 = (USB_BASE + 0x50) as *mut u32;

const ISTR_CTR: u16 = 1 << 15;
const ISTR_ERR: u16 = 1 << 13;
const ISTR_WKUP: u16 = 1 << 12;
const ISTR_SUSP: u16 = 1 << 11;
const ISTR_RESET: u16 = 1 << 10;
const ISTR_EP_ID: u16 = 0x000f;

const CNTR_FRES: u16 = 1 << 0;
const CNTR_CTRM: u16 = 1 << 15;
const CNTR_ERRM: u16 = 1 << 13;
const CNTR_WKUPM: u16 = 1 << 12;
const CNTR_SUSPM: u16 = 1 << 11;
const CNTR_RESETM: u16 = 1 << 10;
const DADDR_EF: u16 = 1 << 7;

const EP_CTR_RX: u16 = 1 << 15;
const EP_DTOG_RX: u16 = 1 << 14;
const EP_STAT_RX: u16 = 0b11 << 12;
const EP_SETUP: u16 = 1 << 11;
const EP_TYPE: u16 = 0b11 << 9;
const EP_KIND: u16 = 1 << 8;
const EP_TYPE_CONTROL: u16 = 0b01 << 9;
const EP_TYPE_BULK: u16 = 0b00 << 9;
#[allow(dead_code)] // Added cuz compiler generates warning that interrupt is unused.
const EP_TYPE_INTERRUPT: u16 = 0b11 << 9;
const EP_CTR_TX: u16 = 1 << 7;
const EP_DTOG_TX: u16 = 1 << 6;
const EP_STAT_TX: u16 = 0b11 << 4;
const EP_ADDR: u16 = 0x0f;

const EP_STAT_DISABLED: u16 = 0b00;
const EP_STAT_STALL: u16 = 0b01;
const EP_STAT_NAK: u16 = 0b10;
const EP_STAT_VALID: u16 = 0b11;

const EP0: usize = 0;
const EP1: usize = 1;
const EP2: usize = 2;

const EP_MAX_PACKET: usize = 64;

struct PmaState {
    sysex_rx: SysexReceiver,
    dmx_parser: DmxParser,
}

impl PmaState {
    const fn new() -> Self {
        Self {
            sysex_rx: SysexReceiver::new(),
            dmx_parser: DmxParser::new(),
        }
    }
}

struct PmaStateCell(UnsafeCell<PmaState>);
unsafe impl Sync for PmaStateCell {}

static PMA_STATE: PmaStateCell = PmaStateCell(UnsafeCell::new(PmaState::new()));

#[inline(always)]
fn uses_ep2_out(cfg: &UsbDeviceConfig, mode: UsbMode) -> bool {
    cfg.use_ep2_for_out || mode == UsbMode::EnttecDmxPro
}

fn pma_layout(cfg: &UsbDeviceConfig, mode: UsbMode) -> (u16, u16, u16, u16, u16) {
    if uses_ep2_out(cfg, mode) {
        // EP0 64b (or 8b), EP1 IN 64b, EP2 OUT 64b
        (0x40, 0x80, 0xc0, 0x00, 0x100)
    } else {
        // Launchpad MK2 / Pro MIDI: EP0 8b, EP1 IN 64b, EP1 OUT 64b
        (0x40, 0x48, 0x90, 0x50, 0x00)
    }
}

pub fn init(cfg: &'static UsbDeviceConfig) {
    init_with_mode(cfg, UsbMode::Midi);
}

pub fn init_with_mode(cfg: &'static UsbDeviceConfig, mode: UsbMode) {
    cortex_m::peripheral::NVIC::mask(pac::Interrupt::USB_LP_CAN1_RX0);
    control().init_with_mode(cfg, mode);

    pac::RCC.cfgr().modify(|w| {
        w.set_usbpre(pac::rcc::vals::Usbpre::DIV1_5);
    });
    pac::RCC.apb1enr().modify(|w| w.set_usben(true));
    pac::RCC.apb1rstr().modify(|w| w.set_usbrst(true));
    for _ in 0..16 {
        cortex_m::asm::nop();
    }
    pac::RCC.apb1rstr().modify(|w| w.set_usbrst(false));

    unsafe {
        write16(USB_CNTR, CNTR_FRES);
        for _ in 0..128 {
            cortex_m::asm::nop();
        }
        write16(USB_CNTR, 0);
        write16(USB_ISTR, 0);
        write16(USB_BTABLE, 0);
        reset_bus();
        write16(
            USB_CNTR,
            CNTR_CTRM | CNTR_ERRM | CNTR_WKUPM | CNTR_SUSPM | CNTR_RESETM,
        );

        cortex_m::peripheral::NVIC::unpend(pac::Interrupt::USB_LP_CAN1_RX0);
        let mut p = cortex_m::Peripherals::steal();
        p.NVIC.set_priority(pac::Interrupt::USB_LP_CAN1_RX0, 0x20); // Priority 2
        cortex_m::peripheral::NVIC::unmask(pac::Interrupt::USB_LP_CAN1_RX0);
    }
}

pub fn reinit_mode(mode: UsbMode) {
    let (cfg, current) = cortex_m::interrupt::free(|_| {
        let ctrl = control();
        (ctrl.cfg, ctrl.mode)
    });
    let Some(cfg) = cfg else {
        return;
    };
    if current == mode {
        return;
    }

    cortex_m::peripheral::NVIC::mask(pac::Interrupt::USB_LP_CAN1_RX0);
    unsafe {
        // Hold USB peripheral in reset and power-down during soft disconnect.
        write16(USB_CNTR, CNTR_FRES | (1 << 1));
    }

    #[cfg(any(feature = "launchpad-mk2", feature = "launchpad-pro"))]
    {
        // PA10 controls the external USB D+ pull-up on Launchpad MK2 / Pro.
        pac::GPIOA
            .brr()
            .write_value(pac::gpio::regs::Brr(1 << 10));
        cortex_m::asm::delay(1_800_000);
        init_with_mode(cfg, mode);
        pac::GPIOA
            .bsrr()
            .write_value(pac::gpio::regs::Bsrr(1 << 10));
    }

    #[cfg(not(any(feature = "launchpad-mk2", feature = "launchpad-pro")))]
    {
        init_with_mode(cfg, mode);
    }
}

pub fn poll() {
    loop {
        let istr = unsafe { read16(USB_ISTR) };

        if istr & ISTR_RESET != 0 {
            unsafe {
                write16(USB_ISTR, !ISTR_RESET);
                reset_bus();
            }
            continue;
        }

        if istr & (ISTR_ERR | ISTR_WKUP | ISTR_SUSP) != 0 {
            unsafe {
                write16(USB_ISTR, !(istr & (ISTR_ERR | ISTR_WKUP | ISTR_SUSP)));
            }
            continue;
        }

        if istr & ISTR_CTR == 0 {
            break;
        }

        let ep = (istr & ISTR_EP_ID) as usize;
        service_ep(ep);
    }

    pump_tx();
}

pub fn pump_tx() {
    let ctrl = control();
    if ctrl.configuration == 0 {
        return;
    }

    if ep_reg(EP1) & EP_STAT_TX == (EP_STAT_VALID << 4) {
        return;
    }

    let Some(cfg) = ctrl.cfg else {
        return;
    };
    let (_, _, ep1_tx_addr, _, _) = pma_layout(cfg, ctrl.mode);

    let mut payload = [0u8; EP_MAX_PACKET];
    let count = match ctrl.mode {
        UsbMode::Midi => {
            let q = queues();
            let mut n = 0;
            while n + 4 <= EP_MAX_PACKET {
                let Some(packet) = q.midi_tx.pop() else {
                    break;
                };
                payload[n..n + 4].copy_from_slice(&packet.data);
                n += 4;
            }
            n
        }
        UsbMode::EnttecDmxPro => dmx_queues().pop_ftdi_in_packet(&mut payload),
    };

    if count > 0 {
        write_ep_tx(EP1, ep1_tx_addr, &payload[..count]);
        unsafe {
            set_tx_stat(EP1, EP_STAT_VALID);
        }
    }
}

unsafe fn reset_bus() {
    let ctrl = control();
    let Some(cfg) = ctrl.cfg else {
        return;
    };
    let mode = ctrl.mode;
    ctrl.init_with_mode(cfg, mode);

    reset_transport_session();

    let (ep0_rx_addr, ep0_tx_addr, ep1_tx_addr, ep1_rx_addr, ep2_rx_addr) = pma_layout(cfg, mode);
    let ep2_out = uses_ep2_out(cfg, mode);

    unsafe {
        write16(USB_DADDR, DADDR_EF);
        write16(USB_BTABLE, 0);

        let ep0_size = cfg.ep0_max_packet_size as usize;
        set_btable(EP0, ep0_tx_addr, 0, ep0_rx_addr, rx_count(ep0_size));

        if ep2_out {
            set_btable(EP1, ep1_tx_addr, 0, 0, 0);
            set_btable(EP2, 0, 0, ep2_rx_addr, rx_count(EP_MAX_PACKET));

            set_ep_reg(EP0, EP_TYPE_CONTROL | 0, EP_STAT_NAK, EP_STAT_VALID);
            set_ep_reg(EP1, EP_TYPE_BULK | 1, EP_STAT_DISABLED, EP_STAT_DISABLED);
            set_ep_reg(EP2, EP_TYPE_BULK | 2, EP_STAT_DISABLED, EP_STAT_DISABLED);
        } else {
            set_btable(EP1, ep1_tx_addr, 0, ep1_rx_addr, rx_count(EP_MAX_PACKET));

            set_ep_reg(EP0, EP_TYPE_CONTROL | 0, EP_STAT_NAK, EP_STAT_VALID);
            set_ep_reg(EP1, EP_TYPE_BULK | 1, EP_STAT_DISABLED, EP_STAT_DISABLED);
        }
    }
}

fn reset_transport_session() {
    queues().clear();
    dmx_queues().clear();
    unsafe {
        let pma = &mut *PMA_STATE.0.get();
        pma.sysex_rx.reset();
        pma.dmx_parser.reset();
    }
}

unsafe fn reset_data_endpoint(ep: usize) {
    unsafe {
        set_tx_stat(ep, EP_STAT_DISABLED);
        set_rx_stat(ep, EP_STAT_DISABLED);
        let register = ep_reg(ep);
        write16(
            (USB_BASE + ep * 4) as *mut u32,
            (register & (EP_TYPE | EP_KIND | EP_ADDR)) | (register & (EP_DTOG_RX | EP_DTOG_TX)),
        );
        write_pma_u16((ep * 8 + 2) as u16, 0);
    }
}

fn service_ep(ep: usize) {
    let mut reg = ep_reg(ep);
    let ctrl = control();
    let Some(cfg) = ctrl.cfg else {
        return;
    };
    let (ep0_rx_addr, ep0_tx_addr, _ep1_tx_addr, ep1_rx_addr, ep2_rx_addr) =
        pma_layout(cfg, ctrl.mode);
    let ep2_out = uses_ep2_out(cfg, ctrl.mode);

    if reg & EP_CTR_RX != 0 {
        if ep == EP0 {
            if reg & EP_SETUP != 0 {
                let mut setup = [0u8; 8];
                pma_read(ep0_rx_addr, &mut setup);
                unsafe {
                    clear_ctr(EP0, true, true);
                }
                reg &= !EP_CTR_TX;
                process_setup(setup);
            } else {
                unsafe {
                    clear_ctr(EP0, true, false);
                    set_rx_stat(EP0, EP_STAT_VALID);
                }
            }
        } else if (ep == EP1 && !ep2_out) || (ep == EP2 && ep2_out) {
            if ctrl.configuration == 0 {
                unsafe {
                    clear_ctr(ep, true, true);
                    set_rx_stat(ep, EP_STAT_DISABLED);
                }
                return;
            }
            let rx_count_offset = (ep * 8 + 6) as u16;
            let rx_bytes = (unsafe { read_pma_u16(rx_count_offset) } & 0x03ff) as usize;
            let mut buf = [0u8; EP_MAX_PACKET];
            let len = rx_bytes.min(EP_MAX_PACKET);
            let rx_addr = if ep == EP1 { ep1_rx_addr } else { ep2_rx_addr };
            pma_read(rx_addr, &mut buf[..len]);

            unsafe {
                clear_ctr(ep, true, false);
                write_pma_u16(rx_count_offset, rx_count(EP_MAX_PACKET));
                set_rx_stat(ep, EP_STAT_VALID);
            }

            process_rx_packets(&buf[..len]);
        }
    }

    if reg & EP_CTR_TX != 0 {
        unsafe {
            clear_ctr(ep, false, true);
        }

        if ep == EP0 {
            let ctrl = control();
            if let Some(address) = ctrl.pending_address.take() {
                unsafe {
                    write16(USB_DADDR, DADDR_EF | (address as u16));
                }
            } else if let Some((ptr, len)) = next_ep0_chunk(ctrl) {
                let slice = if len == 0 || ptr.is_null() {
                    &[]
                } else {
                    unsafe { core::slice::from_raw_parts(ptr, len) }
                };
                write_ep0_tx(EP0, ep0_tx_addr, slice);
                unsafe {
                    set_tx_stat(EP0, EP_STAT_VALID);
                    set_rx_stat(EP0, EP_STAT_VALID);
                }
            } else {
                unsafe {
                    set_tx_stat(EP0, EP_STAT_NAK);
                    set_rx_stat(EP0, EP_STAT_VALID);
                }
            }
        } else if ep == EP1 {
            pump_tx();
        }
    }
}

fn process_setup(setup: [u8; 8]) {
    let ctrl = control();
    let Some(cfg) = ctrl.cfg else {
        return;
    };
    let (_, ep0_tx_addr, _, _, _) = pma_layout(cfg, ctrl.mode);
    let ep2_out = uses_ep2_out(cfg, ctrl.mode);

    match handle_setup_request(setup) {
        SetupAction::SendPacket { data, len } => {
            let slice = if len == 0 || data.is_null() {
                &[]
            } else {
                unsafe { core::slice::from_raw_parts(data, len) }
            };
            write_ep0_tx(EP0, ep0_tx_addr, slice);
            unsafe {
                set_tx_stat(EP0, EP_STAT_VALID);
                set_rx_stat(EP0, EP_STAT_VALID);
            }
        }
        SetupAction::StatusIn => {
            write_ep0_tx(EP0, ep0_tx_addr, &[]);
            unsafe {
                set_tx_stat(EP0, EP_STAT_VALID);
                set_rx_stat(EP0, EP_STAT_VALID);
            }
        }
        SetupAction::Stall => unsafe {
            set_tx_stat(EP0, EP_STAT_STALL);
            set_rx_stat(EP0, EP_STAT_STALL);
        },
        SetupAction::ConfigurationChanged(cfg_val) => {
            unsafe {
                reset_data_endpoint(EP1);
                if ep2_out {
                    reset_data_endpoint(EP2);
                }
                let out_ep = if ep2_out { EP2 } else { EP1 };
                write_pma_u16((out_ep * 8 + 6) as u16, rx_count(EP_MAX_PACKET));
            }
            if cfg_val != 0 {
                unsafe {
                    if ep2_out {
                        set_tx_stat(EP1, EP_STAT_NAK);
                        set_rx_stat(EP2, EP_STAT_VALID);
                    } else {
                        set_tx_stat(EP1, EP_STAT_NAK);
                        set_rx_stat(EP1, EP_STAT_VALID);
                    }
                }
                pump_tx();
            } else {
                reset_transport_session();
            }
            write_ep0_tx(EP0, ep0_tx_addr, &[]);
            unsafe {
                set_tx_stat(EP0, EP_STAT_VALID);
                set_rx_stat(EP0, EP_STAT_VALID);
            }
        }
    }
}

fn write_ep0_tx(ep: usize, addr: u16, data: &[u8]) {
    pma_write(addr, data);
    unsafe {
        write_pma_u16((ep * 8 + 2) as u16, data.len() as u16);
    }
}

fn process_rx_packets(buf: &[u8]) {
    let pma = unsafe { &mut *PMA_STATE.0.get() };

    match control().mode {
        UsbMode::Midi => {
            let q = queues();
            for chunk in buf.chunks_exact(4) {
                parse_usb_midi_packet(
                    chunk,
                    &mut pma.sysex_rx,
                    &mut |event| q.midi_rx.push(event),
                    &mut |msg| q.sysex_rx.push(msg),
                );
            }
        }
        UsbMode::EnttecDmxPro => {
            let dq = dmx_queues();
            pma.dmx_parser.parse_into_queues(buf, dq);
            if !dq.tx_is_empty() {
                pump_tx();
            }
        }
    }
}

#[unsafe(export_name = "USB_LP_CAN1_RX0")]
pub extern "C" fn usb_lp_can_rx0_handler() {
    poll();
}

fn write_ep_tx(ep: usize, addr: u16, data: &[u8]) {
    pma_write(addr, data);
    unsafe {
        write_pma_u16((ep * 8 + 2) as u16, data.len() as u16);
    }
}

unsafe fn set_btable(ep: usize, tx_addr: u16, tx_count: u16, rx_addr: u16, rx_count: u16) {
    let base = (ep * 8) as u16;
    unsafe {
        write_pma_u16(base, tx_addr);
        write_pma_u16(base + 2, tx_count);
        write_pma_u16(base + 4, rx_addr);
        write_pma_u16(base + 6, rx_count);
    }
}

fn rx_count(size: usize) -> u16 {
    if size <= 62 {
        (((size as u16 + 1) / 2) << 10) & 0x7c00
    } else {
        0x8000 | ((((size as u16 + 31) / 32) - 1) << 10)
    }
}

fn ep_reg(ep: usize) -> u16 {
    unsafe { read16((USB_BASE + ep * 4) as *mut u32) }
}

unsafe fn set_ep_reg(ep: usize, base: u16, tx_stat: u16, rx_stat: u16) {
    unsafe {
        write16((USB_BASE + ep * 4) as *mut u32, base);
        set_tx_stat(ep, tx_stat);
        set_rx_stat(ep, rx_stat);
    }
}

unsafe fn set_tx_stat(ep: usize, stat: u16) {
    unsafe {
        let reg = ep_reg(ep);
        let value = (reg & (EP_CTR_RX | EP_CTR_TX | EP_TYPE | EP_KIND | EP_ADDR))
            | ((reg & EP_STAT_TX) ^ (stat << 4));
        write16((USB_BASE + ep * 4) as *mut u32, value);
    }
}

unsafe fn set_rx_stat(ep: usize, stat: u16) {
    unsafe {
        let reg = ep_reg(ep);
        let value = (reg & (EP_CTR_RX | EP_CTR_TX | EP_TYPE | EP_KIND | EP_ADDR))
            | ((reg & EP_STAT_RX) ^ (stat << 12));
        write16((USB_BASE + ep * 4) as *mut u32, value);
    }
}

unsafe fn clear_ctr(ep: usize, rx: bool, tx: bool) {
    unsafe {
        let mut reg = ep_reg(ep);
        reg &= !(EP_DTOG_RX | EP_DTOG_TX | EP_STAT_RX | EP_STAT_TX);
        if rx {
            reg &= !EP_CTR_RX;
        }
        if tx {
            reg &= !EP_CTR_TX;
        }
        write16((USB_BASE + ep * 4) as *mut u32, reg);
    }
}

fn pma_write(addr: u16, data: &[u8]) {
    for (index, chunk) in data.chunks(2).enumerate() {
        let lo = chunk[0] as u16;
        let hi = if chunk.len() > 1 {
            (chunk[1] as u16) << 8
        } else {
            0
        };
        unsafe {
            write_pma_u16(addr + (index as u16) * 2, lo | hi);
        }
    }
}

fn pma_read(addr: u16, data: &mut [u8]) {
    for (index, chunk) in data.chunks_mut(2).enumerate() {
        let word = unsafe { read_pma_u16(addr + (index as u16) * 2) };
        chunk[0] = word as u8;
        if chunk.len() > 1 {
            chunk[1] = (word >> 8) as u8;
        }
    }
}

unsafe fn read_pma_u16(offset: u16) -> u16 {
    unsafe { ptr::read_volatile((USB_PMA + offset as usize * 2) as *const u16) }
}

unsafe fn write_pma_u16(offset: u16, value: u16) {
    unsafe {
        ptr::write_volatile((USB_PMA + offset as usize * 2) as *mut u16, value);
    }
}

unsafe fn read16(reg: *mut u32) -> u16 {
    unsafe { ptr::read_volatile(reg as *const u16) }
}

unsafe fn write16(reg: *mut u32, value: u16) {
    unsafe {
        ptr::write_volatile(reg as *mut u16, value);
    }
}
