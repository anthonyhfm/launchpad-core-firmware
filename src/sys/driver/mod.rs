// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use core::cell::UnsafeCell;
use core::mem;
use crate::sys::midi::MidiPort;

pub mod common;

#[cfg(feature = "launchpad-x")]
#[path = "launchpad-x/mod.rs"]
pub mod launchpad_x;

#[cfg(feature = "launchpad-mini-mk3")]
#[path = "launchpad-mini-mk3/mod.rs"]
pub mod launchpad_mini_mk3;

#[cfg(feature = "launchpad-mk2")]
#[path = "launchpad-mk2/mod.rs"]
pub mod launchpad_mk2;

#[cfg(feature = "launchpad-pro")]
#[path = "launchpad-pro/mod.rs"]
pub mod launchpad_pro;

#[cfg(feature = "launchpad-pro-mk3")]
#[path = "launchpad-pro-mk3/mod.rs"]
pub mod launchpad_pro_mk3;

#[cfg(any(feature = "launchpad-s", feature = "launchpad-mini-mk1"))]
#[path = "launchpad-s-and-mini/mod.rs"]
pub mod launchpad_s_and_mini;

#[cfg(feature = "launchpad-pro-mk3")]
#[derive(Clone, Copy)]
pub struct M0ProbeResult {
    pub status: u8,
    pub read_status: u8,
    pub ack: u8,
    pub baud: u32,
    pub pid: u16,
    pub blid: u8,
    pub vector: [u8; 16],
    pub vector_len: u8,
}

#[cfg(feature = "launchpad-pro-mk3")]
impl M0ProbeResult {
    pub const fn new() -> Self {
        Self {
            status: 1,
            read_status: 5,
            ack: 0,
            baud: 0,
            pid: 0,
            blid: 0xff,
            vector: [0; 16],
            vector_len: 0,
        }
    }
}

#[cfg(feature = "launchpad-pro-mk3")]
#[derive(Clone, Copy)]
pub struct M0FirmwareStatus {
    pub status: u8,
    pub kind: u8,
    pub version_major: u8,
    pub version_minor: u8,
    pub version_patch: u8,
    pub probe: M0ProbeResult,
}

#[cfg(feature = "launchpad-pro-mk3")]
impl M0FirmwareStatus {
    pub const fn unknown() -> Self {
        Self {
            status: 1,
            kind: 2,
            version_major: 0,
            version_minor: 0,
            version_patch: 0,
            probe: M0ProbeResult::new(),
        }
    }
}

#[cfg(feature = "launchpad-pro-mk3")]
#[derive(Clone, Copy)]
pub struct RoadrunnerStats {
    pub fast_frames: u32,
    pub commits: u32,
    pub rx_overruns: u32,
}

#[cfg(feature = "launchpad-pro-mk3")]
#[derive(Clone, Copy)]
pub struct FlashInfo {
    pub present: bool,
    pub jedec_id: [u8; 3],
    pub status1: u8,
}

pub trait Driver {
    fn set_rgb_led(&mut self, index: u8, r: u8, g: u8, b: u8);
    fn set_led(&mut self, index: u8, color: u32) {
        self.set_rgb_led(
            index,
            ((color >> 18) & 0x3f) as u8,
            ((color >> 10) & 0x3f) as u8,
            ((color >> 2) & 0x3f) as u8,
        );
    }
    fn fill(&mut self, color: u32);
    fn brightness(&mut self) -> u8;
    fn set_brightness(&mut self, brightness: u8);
    fn send_midi(&mut self, port: MidiPort, data: &[u8]);
    fn flash_size(&mut self) -> u32;
    fn read_flash(&mut self, offset: u32, data: &mut [u8]);
    fn write_flash(&mut self, offset: u32, data: &[u8]);
    fn prepare_settings_storage(&mut self, _wire_size: usize, _validate: fn(&[u8]) -> bool) {}
    fn device_id(&self) -> u8;
    fn highspeed_leds_enabled(&self) -> bool {
        false
    }

    #[cfg(feature = "launchpad-pro-mk3")]
    fn cached_m0_firmware_status(&mut self) -> Option<M0FirmwareStatus> {
        None
    }
    #[cfg(feature = "launchpad-pro-mk3")]
    fn refresh_m0_firmware_status(&mut self) -> Option<M0FirmwareStatus> {
        None
    }
    #[cfg(feature = "launchpad-pro-mk3")]
    fn flash_info(&mut self) -> Option<FlashInfo> {
        None
    }
    #[cfg(feature = "launchpad-pro-mk3")]
    fn roadrunner_stats(&mut self) -> Option<Option<RoadrunnerStats>> {
        None
    }
}

struct DriverSlot {
    ptr: UnsafeCell<Option<*mut dyn Driver>>,
}

unsafe impl Sync for DriverSlot {}

impl DriverSlot {
    const fn new() -> Self {
        Self {
            ptr: UnsafeCell::new(None),
        }
    }

    fn install(&self, driver: &mut dyn Driver) {
        unsafe {
            let erased: *mut dyn Driver =
                mem::transmute::<&mut dyn Driver, *mut dyn Driver>(driver);
            *self.ptr.get() = Some(erased);
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut dyn Driver) -> R) -> Option<R> {
        unsafe {
            let slot = &mut *self.ptr.get();
            slot.as_mut().map(|ptr| f(&mut **ptr))
        }
    }
}

static DRIVER: DriverSlot = DriverSlot::new();

pub fn install(driver: &mut dyn Driver) {
    DRIVER.install(driver);
}

pub fn with<R>(f: impl FnOnce(&mut dyn Driver) -> R) -> Option<R> {
    DRIVER.with(f)
}

pub fn set_rgb_led(index: u8, r: u8, g: u8, b: u8) {
    let _ = with(|driver| driver.set_rgb_led(index, r, g, b));
}

pub fn set_led_raw(index: u8, r: u8, g: u8, b: u8) {
    set_rgb_led(index, r, g, b);
}

pub fn set_led(index: u8, color: u32) {
    let _ = with(|driver| {
        driver.set_rgb_led(
            index,
            ((color >> 18) & 0x3f) as u8,
            ((color >> 10) & 0x3f) as u8,
            ((color >> 2) & 0x3f) as u8,
        )
    });
}

pub fn fill(color: u32) {
    let _ = with(|driver| driver.fill(color));
}

pub fn brightness() -> u8 {
    with(|driver| driver.brightness()).unwrap_or(7)
}

pub fn set_brightness(brightness: u8) {
    let _ = with(|driver| driver.set_brightness(brightness));
}

pub fn send_midi(port: MidiPort, data: &[u8]) {
    let _ = with(|driver| driver.send_midi(port, data));
}

pub fn flash_size() -> u32 {
    with(|driver| driver.flash_size()).unwrap_or(0)
}

pub fn read_flash(offset: u32, data: &mut [u8]) {
    let _ = with(|driver| driver.read_flash(offset, data));
}

pub fn write_flash(offset: u32, data: &[u8]) {
    let _ = with(|driver| driver.write_flash(offset, data));
}

pub fn prepare_settings_storage(wire_size: usize, validate: fn(&[u8]) -> bool) {
    let _ = with(|driver| driver.prepare_settings_storage(wire_size, validate));
}

pub fn device_id() -> u8 {
    with(|driver| driver.device_id()).unwrap_or(0)
}

pub fn highspeed_leds_enabled() -> bool {
    with(|driver| driver.highspeed_leds_enabled()).unwrap_or(false)
}

#[cfg(feature = "launchpad-pro-mk3")]
pub fn cached_m0_firmware_status() -> Option<M0FirmwareStatus> {
    with(|driver| driver.cached_m0_firmware_status()).flatten()
}

#[cfg(feature = "launchpad-pro-mk3")]
pub fn refresh_m0_firmware_status() -> Option<M0FirmwareStatus> {
    with(|driver| driver.refresh_m0_firmware_status()).flatten()
}

#[cfg(feature = "launchpad-pro-mk3")]
pub fn flash_info() -> Option<FlashInfo> {
    with(|driver| driver.flash_info()).flatten()
}

#[cfg(feature = "launchpad-pro-mk3")]
pub fn roadrunner_stats() -> Option<Option<RoadrunnerStats>> {
    with(|driver| driver.roadrunner_stats()).flatten()
}
