// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister
// Copyright (C) 2026 ZephyrCodesStuff

#![allow(dead_code)]

#[path = "../src/sys/driver/common/usb/control.rs"]
mod control;
#[path = "../src/sys/driver/common/usb/descriptors.rs"]
mod descriptors;
#[path = "../src/sys/driver/common/usb/dmx.rs"]
mod dmx;

#[derive(Copy, Clone)]
pub struct UsbDeviceConfig {
    pub vendor_id: u16,
    pub product_id: u16,
    pub bcd_device: u16,
    pub ep0_max_packet_size: u8,
    pub max_power_ma: u16,
    pub manufacturer: &'static str,
    pub product: &'static str,
    pub serial_number: &'static str,
    pub port1_name: &'static str,
    pub port2_name: &'static str,
    pub use_ep2_for_out: bool,
}

static CONFIG: UsbDeviceConfig = UsbDeviceConfig {
    vendor_id: 0x1235,
    product_id: 0x0103,
    bcd_device: 0x0200,
    ep0_max_packet_size: 64,
    max_power_ma: 500,
    manufacturer: "CoreFW",
    product: "Launchpad X",
    serial_number: "COREFW-LPX",
    port1_name: "DAW",
    port2_name: "MIDI",
    use_ep2_for_out: false,
};

fn read_control_transfer(state: &mut control::ControlState, setup: [u8; 8]) -> Vec<u8> {
    let mut out = Vec::new();
    match control::handle_setup(state, setup) {
        control::SetupAction::SendPacket { data, len } => {
            if len > 0 && !data.is_null() {
                out.extend_from_slice(unsafe { core::slice::from_raw_parts(data, len) });
            }
        }
        _ => panic!("expected SendPacket for setup {setup:?}"),
    }
    while let Some((data, len)) = control::next_ep0_chunk(state) {
        if len > 0 && !data.is_null() {
            out.extend_from_slice(unsafe { core::slice::from_raw_parts(data, len) });
        }
    }
    out
}

fn decode_utf16le_string_desc(desc: &[u8]) -> String {
    assert!(desc.len() >= 2);
    assert_eq!(desc[0] as usize, desc.len());
    assert_eq!(desc[1], descriptors::DESC_STRING);
    let mut s = String::new();
    for pair in desc[2..].chunks_exact(2) {
        assert_eq!(pair[1], 0);
        s.push(pair[0] as char);
    }
    s
}

#[test]
fn enttec_device_and_config_descriptors_match_ft232r() {
    let mut state = control::ControlState::new();
    state.init_with_mode(&CONFIG, descriptors::UsbMode::EnttecDmxPro);

    // GET_DESCRIPTOR(DEVICE)
    let dev = read_control_transfer(&mut state, [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 18, 0]);
    assert_eq!(dev.len(), 18);
    assert_eq!(u16::from_le_bytes([dev[8], dev[9]]), 0x0403);
    assert_eq!(u16::from_le_bytes([dev[10], dev[11]]), 0x6001);
    assert_eq!(u16::from_le_bytes([dev[12], dev[13]]), 0x0600);

    // GET_DESCRIPTOR(CONFIGURATION)
    let cfg = read_control_transfer(&mut state, [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 0xff, 0]);
    assert_eq!(cfg.len(), descriptors::ENTTEC_CONFIG_DESCRIPTOR_LEN);
    assert_eq!(cfg[4], 1); // 1 interface
    // Interface class/subclass/protocol = 0xFF/0xFF/0xFF
    assert_eq!(&cfg[14..17], &[0xff, 0xff, 0xff]);
    // EP1 IN = 0x81, EP2 OUT = 0x02
    assert_eq!(cfg[20], 0x81);
    assert_eq!(cfg[27], 0x02);

    // String descriptors
    let mfr = read_control_transfer(&mut state, [0x80, 0x06, 0x01, 0x03, 0x09, 0x04, 64, 0]);
    let prod = read_control_transfer(&mut state, [0x80, 0x06, 0x02, 0x03, 0x09, 0x04, 64, 0]);
    let ser = read_control_transfer(&mut state, [0x80, 0x06, 0x03, 0x03, 0x09, 0x04, 64, 0]);
    assert_eq!(decode_utf16le_string_desc(&mfr), "ENTTEC");
    assert_eq!(decode_utf16le_string_desc(&prod), "DMX USB PRO");
    assert_eq!(decode_utf16le_string_desc(&ser), "EN000001");

    // Port strings 4 & 5 are not present in Enttec mode.
    assert!(matches!(
        control::handle_setup(&mut state, [0x80, 0x06, 0x04, 0x03, 0x09, 0x04, 64, 0]),
        control::SetupAction::Stall
    ));
}

#[test]
fn ftdi_vendor_control_requests_work_in_enttec_mode_and_stall_in_midi_mode() {
    let mut state = control::ControlState::new();
    state.init_with_mode(&CONFIG, descriptors::UsbMode::EnttecDmxPro);

    for req in [
        control::FTDI_SIO_RESET,
        control::FTDI_SIO_MODEM_CTRL,
        control::FTDI_SIO_SET_FLOW_CTRL,
        control::FTDI_SIO_SET_BAUD_RATE,
        control::FTDI_SIO_SET_DATA,
        control::FTDI_SIO_SET_EVENT_CHAR,
        control::FTDI_SIO_SET_ERROR_CHAR,
        control::FTDI_SIO_SET_BITMODE,
    ] {
        assert!(matches!(
            control::handle_setup(&mut state, [0x40, req, 0, 0, 0, 0, 0, 0]),
            control::SetupAction::StatusIn
        ));
    }

    // Set and read back latency timer
    assert!(matches!(
        control::handle_setup(&mut state, [0x40, control::FTDI_SIO_SET_LATENCY_TIMER, 2, 0, 0, 0, 0, 0]),
        control::SetupAction::StatusIn
    ));
    let latency = read_control_transfer(
        &mut state,
        [0xc0, control::FTDI_SIO_GET_LATENCY_TIMER, 0, 0, 0, 0, 1, 0],
    );
    assert_eq!(latency, vec![2]);

    // Modem status returns [0x01, 0x60]
    let modem = read_control_transfer(
        &mut state,
        [0xc0, control::FTDI_SIO_GET_MODEM_STATUS, 0, 0, 0, 0, 2, 0],
    );
    assert_eq!(modem, vec![0x01, 0x60]);

    // EEPROM read returns 2 bytes
    let eeprom = read_control_transfer(
        &mut state,
        [0xc0, control::FTDI_SIO_READ_EEPROM, 0, 0, 0, 0, 2, 0],
    );
    assert_eq!(eeprom, vec![0xff, 0xff]);

    // In MIDI mode, FTDI vendor requests must STALL
    state.init_with_mode(&CONFIG, descriptors::UsbMode::Midi);
    assert!(matches!(
        control::handle_setup(&mut state, [0x40, control::FTDI_SIO_RESET, 0, 0, 0, 0, 0, 0]),
        control::SetupAction::Stall
    ));
}

#[test]
fn enttec_widget_params_and_serial_replies_and_ftdi_in_framing() {
    let mut parser = dmx::DmxParser::new();
    let mut queues = dmx::DmxQueues::new();

    // Label 3: GET_WIDGET_PARAMS
    let get_params = [0x7e, 3, 2, 0, 0, 0, 0xe7];
    parser.parse_into_queues(&get_params, &mut queues);
    let mut pkt = [0u8; 64];
    let n = queues.pop_ftdi_in_packet(&mut pkt);
    assert_eq!(
        &pkt[..n],
        &[0x01, 0x60, 0x7e, 3, 5, 0, 44, 1, 9, 1, 40, 0xe7]
    );
    assert_eq!(queues.pop_ftdi_in_packet(&mut pkt), 0);

    // Label 4: SET_WIDGET_PARAMS (break=12, mab=2, rate=30), then read back with Label 3
    let set_params = [0x7e, 4, 5, 0, 0, 0, 12, 2, 30, 0xe7];
    parser.parse_into_queues(&set_params, &mut queues);
    assert_eq!(queues.pop_ftdi_in_packet(&mut pkt), 0);

    parser.parse_into_queues(&get_params, &mut queues);
    let n = queues.pop_ftdi_in_packet(&mut pkt);
    assert_eq!(
        &pkt[..n],
        &[0x01, 0x60, 0x7e, 3, 5, 0, 44, 1, 12, 2, 30, 0xe7]
    );

    // Label 10: GET_WIDGET_SERIAL
    let get_serial = [0x7e, 10, 0, 0, 0xe7];
    parser.parse_into_queues(&get_serial, &mut queues);
    let n = queues.pop_ftdi_in_packet(&mut pkt);
    assert_eq!(
        &pkt[..n],
        &[0x01, 0x60, 0x7e, 10, 4, 0, 0x01, 0x00, 0x00, 0x00, 0xe7]
    );
}

#[test]
fn enttec_send_dmx_512_universe_split_across_usb_packets() {
    let mut parser = dmx::DmxParser::new();
    let mut queues = dmx::DmxQueues::new();

    // Build a full 512-channel DMX packet (payload len = 513: 1 start code + 512 channels)
    let mut wire = Vec::new();
    wire.extend_from_slice(&[0x00, 0xff, 0x12]); // leading garbage
    wire.push(0x7e);
    wire.push(dmx::LABEL_OUTPUT_ONLY_SEND_DMX);
    let payload_len: u16 = 513;
    wire.extend_from_slice(&payload_len.to_le_bytes());
    wire.push(0x00); // DMX start code 0
    for ch in 0..512usize {
        wire.push((ch % 251) as u8);
    }
    wire.push(0xe7);

    // Feed in 64-byte USB Bulk OUT chunks
    for chunk in wire.chunks(64) {
        parser.parse_into_queues(chunk, &mut queues);
    }

    let frame = queues.take_frame().expect("expected latched DMX frame");
    assert!(queues.take_frame().is_none());
    for ch in 0..dmx::DMX_RGB_CHANNELS {
        assert_eq!(frame.channels[ch], (ch % 251) as u8);
    }

    // Non-zero start code (e.g. RDM 0xCC) should be ignored
    let rdm_pkt = [0x7e, 6, 4, 0, 0xcc, 0xff, 0xff, 0xff, 0xe7];
    parser.parse_into_queues(&rdm_pkt, &mut queues);
    assert!(queues.take_frame().is_none());
}

#[test]
fn dmx_pad_mapping_is_row_major_top_to_bottom_left_to_right() {
    assert_eq!(dmx::pad_index_to_xy(0), 81);  // top-left
    assert_eq!(dmx::pad_index_to_xy(7), 88);  // top-right
    assert_eq!(dmx::pad_index_to_xy(8), 71);  // second row, left
    assert_eq!(dmx::pad_index_to_xy(56), 11); // bottom-left
    assert_eq!(dmx::pad_index_to_xy(63), 18); // bottom-right
}
