// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use core::cell::UnsafeCell;

use crate::driver;
use crate::utils::palette::PALETTE_NOVATION;

pub const SETTINGS_FLASH_SIZE: usize = 6 * 1024;
const SETTINGS_MAGIC: [u8; 6] = *b"COREFW";
const SETTINGS_CRC_OFFSET: usize = SETTINGS_MAGIC.len();
const SETTINGS_CRC_SIZE: usize = 4;
const SETTINGS_DATA_OFFSET: usize = SETTINGS_CRC_OFFSET + SETTINGS_CRC_SIZE;
const BASIC_SETTINGS_SIZE: usize = 7;
const PALETTE_OFFSET: usize = SETTINGS_DATA_OFFSET + BASIC_SETTINGS_SIZE;
const CUSTOM_PALETTE_BYTES: usize = 3 * 3 * 128;
const SETTINGS_WIRE_SIZE: usize = PALETTE_OFFSET + CUSTOM_PALETTE_BYTES;

#[cfg(feature = "launchpad-pro-mk3")]
const _: () = assert!(
    SETTINGS_WIRE_SIZE
        <= crate::sys::driver::common::storage::external::SETTINGS_SIZE as usize
);

#[derive(Copy, Clone)]
pub struct Settings {
    pub brightness: u8,
    pub velocity_enabled: u8,
    pub velocity_curve: u8,
    pub aftertouch_mode: u8,
    pub aftertouch_curve: u8,
    pub palette: u8,
    pub mirror_enabled: u8,
    pub custom_palette: [[[u8; 128]; 3]; 3],
}

impl Settings {
    pub const fn empty() -> Self {
        Self {
            brightness: 0,
            velocity_enabled: 0,
            velocity_curve: 0,
            aftertouch_mode: 0,
            aftertouch_curve: 0,
            palette: 0,
            mirror_enabled: 0,
            custom_palette: [[[0; 128]; 3]; 3],
        }
    }

    pub const fn defaults() -> Self {
        Self {
            brightness: 7,
            velocity_enabled: 0,
            velocity_curve: 1,
            aftertouch_mode: 0,
            aftertouch_curve: 0,
            palette: 0,
            mirror_enabled: 0,
            custom_palette: [[[0; 128]; 3]; 3],
        }
    }

    fn sanitize(&mut self) {
        if self.brightness > 7 {
            self.brightness = 7;
        }
        if self.velocity_enabled > 2 {
            self.velocity_enabled = 0;
        }
        if self.velocity_curve > 2 {
            self.velocity_curve = 1;
        }
        if self.aftertouch_mode > 2 {
            self.aftertouch_mode = 0;
        }
        if self.aftertouch_curve > 2 {
            self.aftertouch_curve = 0;
        }
        if self.palette > 6 {
            self.palette = 0;
        }
        if self.mirror_enabled > 1 {
            self.mirror_enabled = 0;
        }
    }

    fn fill_empty_custom_palettes(&mut self) -> bool {
        let mut changed = false;

        for slot in 0..3 {
            let is_zeroed = self.custom_palette[slot]
                .iter()
                .all(|channel| channel.iter().all(|value| *value == 0));
            let is_erased = self.custom_palette[slot]
                .iter()
                .all(|channel| channel.iter().all(|value| *value == 0xff));

            if !is_zeroed && !is_erased {
                continue;
            }

            for index in 0..128 {
                let (r, g, b) = PALETTE_NOVATION.rgb(index as u8);
                self.custom_palette[slot][0][index] = r;
                self.custom_palette[slot][1][index] = g;
                self.custom_palette[slot][2][index] = b;
            }
            changed = true;
        }

        changed
    }

    fn encode(&self, out: &mut [u8]) {
        out.fill(0);
        out[..SETTINGS_MAGIC.len()].copy_from_slice(&SETTINGS_MAGIC);
        out[SETTINGS_CRC_OFFSET..SETTINGS_DATA_OFFSET].fill(0);
        out[SETTINGS_DATA_OFFSET] = self.brightness;
        out[SETTINGS_DATA_OFFSET + 1] = self.velocity_enabled;
        out[SETTINGS_DATA_OFFSET + 2] = self.velocity_curve;
        out[SETTINGS_DATA_OFFSET + 3] = self.aftertouch_mode;
        out[SETTINGS_DATA_OFFSET + 4] = self.aftertouch_curve;
        out[SETTINGS_DATA_OFFSET + 5] = self.palette;
        out[SETTINGS_DATA_OFFSET + 6] = self.mirror_enabled;

        let mut pos = PALETTE_OFFSET;
        for slot in self.custom_palette {
            for channel in slot {
                out[pos..pos + 128].copy_from_slice(&channel);
                pos += 128;
            }
        }

        let crc = crc32(&out[SETTINGS_DATA_OFFSET..SETTINGS_WIRE_SIZE]);
        out[SETTINGS_CRC_OFFSET..SETTINGS_DATA_OFFSET].copy_from_slice(&crc.to_le_bytes());
    }

    fn decode(input: &[u8]) -> Self {
        let mut settings = Self::defaults();

        settings.brightness = input[SETTINGS_DATA_OFFSET];
        settings.velocity_enabled = input[SETTINGS_DATA_OFFSET + 1];
        settings.velocity_curve = input[SETTINGS_DATA_OFFSET + 2];
        settings.aftertouch_mode = input[SETTINGS_DATA_OFFSET + 3];
        settings.aftertouch_curve = input[SETTINGS_DATA_OFFSET + 4];
        settings.palette = input[SETTINGS_DATA_OFFSET + 5];
        settings.mirror_enabled = input[SETTINGS_DATA_OFFSET + 6];

        if input.len() >= SETTINGS_WIRE_SIZE {
            let mut pos = PALETTE_OFFSET;
            for slot in 0..3 {
                for channel in 0..3 {
                    settings.custom_palette[slot][channel].copy_from_slice(&input[pos..pos + 128]);
                    pos += 128;
                }
            }
        }

        settings.sanitize();
        settings
    }
}

struct SettingsSlot {
    inner: UnsafeCell<Settings>,
}

unsafe impl Sync for SettingsSlot {}

static SETTINGS: SettingsSlot = SettingsSlot {
    inner: UnsafeCell::new(Settings::empty()),
};

pub fn get() -> Settings {
    unsafe { *SETTINGS.inner.get() }
}

pub fn with<R>(f: impl FnOnce(&Settings) -> R) -> R {
    unsafe { f(&*SETTINGS.inner.get()) }
}

pub fn update(f: impl FnOnce(&mut Settings)) {
    unsafe {
        let settings = &mut *SETTINGS.inner.get();
        f(settings);
        settings.sanitize();
    }
}

pub fn load() {
    // Device drivers may need to move settings away from storage owned by the
    // stock firmware before the normal logical settings mapping is accessed.
    driver::prepare_settings_storage(SETTINGS_WIRE_SIZE, validate_wire_image);

    let flash_size = driver::flash_size() as usize;
    let read_len = min_usize(
        min_usize(flash_size, SETTINGS_FLASH_SIZE),
        SETTINGS_WIRE_SIZE,
    );
    let mut buf = [0xff; SETTINGS_WIRE_SIZE];

    if read_len != 0 {
        driver::read_flash(0, &mut buf[..read_len]);
    }

    if read_len < SETTINGS_WIRE_SIZE || !validate_wire_image(&buf) {
        let mut settings = Settings::defaults();
        settings.fill_empty_custom_palettes();

        unsafe {
            *SETTINGS.inner.get() = settings;
        }
        driver::set_brightness(with(|settings| settings.brightness));
        save();
        return;
    }

    let mut settings = Settings::decode(&buf);
    let settings_changed = settings.fill_empty_custom_palettes();

    unsafe {
        *SETTINGS.inner.get() = settings;
    }
    driver::set_brightness(with(|settings| settings.brightness));

    if settings_changed {
        save();
    }
}

pub fn save() {
    let flash_size = driver::flash_size() as usize;
    let write_len = min_usize(
        min_usize(flash_size, SETTINGS_FLASH_SIZE),
        SETTINGS_WIRE_SIZE,
    );
    if write_len == 0 {
        return;
    }

    let mut buf = [0; SETTINGS_WIRE_SIZE];
    with(|settings| settings.encode(&mut buf));
    driver::write_flash(0, &buf[..write_len]);
}

const fn min_usize(a: usize, b: usize) -> usize {
    if a < b { a } else { b }
}

fn validate_wire_image(input: &[u8]) -> bool {
    if input.len() < SETTINGS_WIRE_SIZE {
        return false;
    }

    if input[..SETTINGS_MAGIC.len()] != SETTINGS_MAGIC {
        return false;
    }

    let stored_crc = u32::from_le_bytes([
        input[SETTINGS_CRC_OFFSET],
        input[SETTINGS_CRC_OFFSET + 1],
        input[SETTINGS_CRC_OFFSET + 2],
        input[SETTINGS_CRC_OFFSET + 3],
    ]);
    let computed_crc = crc32(&input[SETTINGS_DATA_OFFSET..SETTINGS_WIRE_SIZE]);

    stored_crc == computed_crc
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;

    for byte in bytes {
        crc ^= *byte as u32;

        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }

    !crc
}
