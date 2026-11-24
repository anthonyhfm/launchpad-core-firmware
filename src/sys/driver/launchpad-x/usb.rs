// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister
// Copyright (C) 2026 ZephyrCodesStuff

pub use crate::sys::driver::common::usb::*;

pub const USB_CONFIG: UsbDeviceConfig = UsbDeviceConfig {
    vendor_id: 0x1235,
    product_id: 0x0103,
    bcd_device: 0x0200,
    ep0_max_packet_size: 64,
    max_power_ma: 500,
    manufacturer: "Focusrite - Novation",
    product: "Launchpad X",
    serial_number: "COREFW-LPX",
    port1_name: "LPX (DAW)",
    port2_name: "LPX (MIDI)",
    use_ep2_for_out: false,
};

pub fn init() {
    crate::sys::driver::common::usb::init(&USB_CONFIG);
}

pub fn init_with_mode(mode: UsbMode) {
    crate::sys::driver::common::usb::init_with_mode(&USB_CONFIG, mode);
}
