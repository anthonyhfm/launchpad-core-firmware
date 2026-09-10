// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use super::UsbDeviceConfig;

pub const DESC_DEVICE: u8 = 0x01;
pub const DESC_CONFIGURATION: u8 = 0x02;
pub const DESC_STRING: u8 = 0x03;
pub const DESC_INTERFACE: u8 = 0x04;
pub const DESC_ENDPOINT: u8 = 0x05;

pub const CS_INTERFACE: u8 = 0x24;
pub const CS_ENDPOINT: u8 = 0x25;

pub const STRING_LANG_ID: [u8; 4] = [4, DESC_STRING, 0x09, 0x04];

pub fn build_device_descriptor(cfg: &UsbDeviceConfig, out: &mut [u8; 18]) {
    out[0] = 18;
    out[1] = DESC_DEVICE;
    out[2] = 0x00; // bcdUSB 2.0 (0x0200)
    out[3] = 0x02;
    out[4] = 0x00; // Device Class (defined at interface level)
    out[5] = 0x00; // SubClass
    out[6] = 0x00; // Protocol
    out[7] = cfg.ep0_max_packet_size;
    out[8] = (cfg.vendor_id & 0xff) as u8;
    out[9] = ((cfg.vendor_id >> 8) & 0xff) as u8;
    out[10] = (cfg.product_id & 0xff) as u8;
    out[11] = ((cfg.product_id >> 8) & 0xff) as u8;
    out[12] = (cfg.bcd_device & 0xff) as u8;
    out[13] = ((cfg.bcd_device >> 8) & 0xff) as u8;
    out[14] = 1; // iManufacturer
    out[15] = 2; // iProduct
    out[16] = 3; // iSerialNumber
    out[17] = 1; // bNumConfigurations
}

pub const CONFIG_DESCRIPTOR_LEN: usize = 129;
pub const MS_HEADER_TOTAL_LEN: u16 = 93;

pub fn build_config_descriptor(cfg: &UsbDeviceConfig, out: &mut [u8; CONFIG_DESCRIPTOR_LEN]) {
    let ep_out_addr = if cfg.use_ep2_for_out { 0x02 } else { 0x01 };
    let ep_attr = 0x02; // 0x02 = Bulk

    let max_power = (cfg.max_power_ma / 2).min(255) as u8;

    let desc: [u8; CONFIG_DESCRIPTOR_LEN] = [
        // 1. Configuration Descriptor (9 bytes)
        9,
        DESC_CONFIGURATION,
        (CONFIG_DESCRIPTOR_LEN & 0xff) as u8,
        ((CONFIG_DESCRIPTOR_LEN >> 8) & 0xff) as u8,
        2, // 2 interfaces (AudioControl + MidiStreaming)
        1, // bConfigurationValue
        0, // iConfiguration
        0x80, // bmAttributes (Bus Powered)
        max_power,

        // 2. Audio Control (AC) Interface Descriptor (9 bytes)
        9,
        DESC_INTERFACE,
        0, // bInterfaceNumber
        0, // bAlternateSetting
        0, // bNumEndpoints
        1, // bInterfaceClass (Audio)
        1, // bInterfaceSubClass (AudioControl)
        0, // bInterfaceProtocol
        0, // iInterface

        // 3. Class-Specific AC Interface Header (9 bytes)
        9,
        CS_INTERFACE,
        1, // HEADER subtype
        0x00, 0x01, // bcdADC (1.00)
        9, 0, // wTotalLength
        1, // bInCollection
        1, // baInterfaceNr(1) = 1 (MIDIStreaming interface number)

        // 4. MIDI Streaming (MS) Interface Descriptor (9 bytes)
        9,
        DESC_INTERFACE,
        1, // bInterfaceNumber
        0, // bAlternateSetting
        2, // bNumEndpoints
        1, // bInterfaceClass (Audio)
        3, // bInterfaceSubClass (MIDISTREAMING)
        0, // bInterfaceProtocol
        0, // iInterface

        // 5. Class-Specific MS Interface Header (7 bytes)
        7,
        CS_INTERFACE,
        1, // MS_HEADER subtype
        0x00, 0x01, // bcdMSC (1.00)
        (MS_HEADER_TOTAL_LEN & 0xff) as u8,
        ((MS_HEADER_TOTAL_LEN >> 8) & 0xff) as u8,

        // 6. MIDI IN Jacks (External 1, External 2, Embedded 1, Embedded 2)
        // 6.1 External IN Jack 1 (ID 1)
        6, CS_INTERFACE, 2, 1, 1, 0,
        // 6.2 External IN Jack 2 (ID 2)
        6, CS_INTERFACE, 2, 1, 2, 0,
        // 6.3 Embedded IN Jack 1 (ID 3, DAW - String Index 4)
        6, CS_INTERFACE, 2, 2, 3, 4,
        // 6.4 Embedded IN Jack 2 (ID 4, MIDI - String Index 5)
        6, CS_INTERFACE, 2, 2, 4, 5,

        // 7. MIDI OUT Jacks (External 1, External 2, Embedded 1, Embedded 2)
        // 7.1 External OUT Jack 1 (ID 5, source Embedded IN 1 ID 3)
        9, CS_INTERFACE, 3, 1, 5, 1, 3, 1, 0,
        // 7.2 External OUT Jack 2 (ID 6, source Embedded IN 2 ID 4)
        9, CS_INTERFACE, 3, 1, 6, 1, 4, 1, 0,
        // 7.3 Embedded OUT Jack 1 (ID 7, source External IN 1 ID 1, DAW - String Index 4)
        9, CS_INTERFACE, 3, 2, 7, 1, 1, 1, 4,
        // 7.4 Embedded OUT Jack 2 (ID 8, source External IN 2 ID 2, MIDI - String Index 5)
        9, CS_INTERFACE, 3, 2, 8, 1, 2, 1, 5,

        // 8. Endpoints
        // 8.1 OUT Endpoint Descriptor (7 bytes)
        7,
        DESC_ENDPOINT,
        ep_out_addr, // OUT endpoint address (0x01 or 0x02)
        ep_attr,
        64, 0, // Max packet size 64 bytes
        0,

        // 8.2 Class-Specific MS Bulk OUT Endpoint Descriptor (6 bytes)
        6,
        CS_ENDPOINT,
        1, // MS_GENERAL
        2, // bNumEmbMIDIJack
        3, // Embedded IN Jack 1 (ID 3)
        4, // Embedded IN Jack 2 (ID 4)

        // 8.3 IN Endpoint Descriptor (7 bytes)
        7,
        DESC_ENDPOINT,
        0x81, // IN endpoint address (0x81 = EP1 IN)
        ep_attr,
        64, 0, // Max packet size 64 bytes
        0,

        // 8.4 Class-Specific MS Bulk IN Endpoint Descriptor (6 bytes)
        6,
        CS_ENDPOINT,
        1, // MS_GENERAL
        2, // bNumEmbMIDIJack
        7, // Embedded OUT Jack 1 (ID 7)
        8, // Embedded OUT Jack 2 (ID 8)
    ];

    out.copy_from_slice(&desc);
}

pub fn encode_string_descriptor(s: &str, out: &mut [u8]) -> usize {
    let len = s.len();
    let total_len = 2 + len * 2;
    if total_len > out.len() {
        return 0;
    }

    out[0] = total_len as u8;
    out[1] = DESC_STRING;

    for (i, byte) in s.bytes().enumerate() {
        out[2 + i * 2] = byte;
        out[3 + i * 2] = 0;
    }

    total_len
}
