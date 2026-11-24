// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

pub use crate::sys::driver::common::usb::*;

pub const USB_CONFIG: UsbDeviceConfig = UsbDeviceConfig {
    vendor_id: 0x1235,
    product_id: 0x0069,
    bcd_device: 0x0100,
    ep0_max_packet_size: 8,
    max_power_ma: 500,
    manufacturer: "Focusrite A.E. Ltd",
    product: "Launchpad MK2",
    serial_number: "COREFW-MK2",
    port1_name: "MK2 (DAW)",
    port2_name: "MK2 (MIDI)",
    use_ep2_for_out: false,
};

pub fn init() {
    crate::sys::driver::common::usb::init(&USB_CONFIG);
}

pub fn init_with_mode(mode: UsbMode) {
    crate::sys::driver::common::usb::init_with_mode(&USB_CONFIG, mode);
}
