// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister

#![allow(dead_code)]
#[path = "../src/sys/driver/common/usb/control.rs"]
mod control;
#[path = "../src/sys/driver/common/usb/descriptors.rs"]
mod descriptors;
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

fn initialized() -> control::ControlState {
    let mut state = control::ControlState::new();
    reset(&mut state);
    state
}
fn reset(state: &mut control::ControlState) {
    state.init(&CONFIG);
}

static CONFIG: UsbDeviceConfig = UsbDeviceConfig {
    vendor_id: 0x1235,
    product_id: 0x0103,
    bcd_device: 0x0200,
    ep0_max_packet_size: 64,
    max_power_ma: 500,
    manufacturer: "test",
    product: "USB abort",
    serial_number: "test",
    port1_name: "DAW",
    port2_name: "MIDI",
    use_ep2_for_out: false,
};

fn first_packet(action: control::SetupAction) -> Vec<u8> {
    match action {
        control::SetupAction::SendPacket { data, len } => {
            assert!(!data.is_null());
            unsafe { core::slice::from_raw_parts(data, len) }.to_vec()
        }
        _ => panic!("expected descriptor data"),
    }
}

fn assert_cancelled(state: &mut control::ControlState) {
    assert_eq!(state.pending_address, None);
    assert!(state.tx_data.is_null());
    assert_eq!((state.tx_len, state.tx_pos, state.tx_zlp), (0, 0, false));
    assert!(control::next_ep0_chunk(state).is_none());
    assert!(matches!(
        control::complete_ep0_in(state, true),
        control::Ep0InCompletion::Ignore
    ));
}

#[test]
fn abort_read_then_set_address_waits_for_its_own_status_completion() {
    for address in [0, 42, 127] {
        let mut state = initialized();
        let first = first_packet(control::handle_setup(
            &mut state,
            [0x80, 6, 0, 2, 0, 0, 255, 0],
        ));
        assert_eq!(first.len(), 64);
        assert!(state.tx_len > state.tx_pos);
        state.abort_transfer();
        assert_cancelled(&mut state);
        assert!(matches!(
            control::handle_setup(&mut state, [0, 5, address, 0, 0, 0, 0, 0]),
            control::SetupAction::StatusIn
        ));
        assert_eq!(state.pending_address, Some(address));
        assert!(matches!(
            control::complete_ep0_in(&mut state, true),
            control::Ep0InCompletion::Ignore
        ));
        assert_eq!(state.pending_address, Some(address));
        assert!(
            matches!(control::complete_ep0_in(&mut state, false), control::Ep0InCompletion::SetAddress(a) if a == address)
        );
        assert!(matches!(
            control::complete_ep0_in(&mut state, false),
            control::Ep0InCompletion::ArmOut
        ));
    }
}

#[test]
fn abort_pending_address_then_descriptor_never_commits_the_old_address() {
    for address in [0, 42] {
        let mut state = initialized();
        control::handle_setup(&mut state, [0, 5, address, 0, 0, 0, 0, 0]);
        assert_eq!(state.pending_address, Some(address));
        state.abort_transfer();
        assert_cancelled(&mut state);
        let data = first_packet(control::handle_setup(
            &mut state,
            [0x80, 6, 0, 1, 0, 0, 255, 0],
        ));
        let mut expected = [0; 18];
        descriptors::build_device_descriptor(&CONFIG, &mut expected);
        assert_eq!(data, expected);
        assert!(matches!(
            control::complete_ep0_in(&mut state, false),
            control::Ep0InCompletion::ArmOut
        ));
        assert_eq!(state.pending_address, None);
    }
}

#[test]
fn abort_zlp_then_replacement_descriptor_starts_at_byte_zero_and_finishes_exactly() {
    let mut state = initialized();
    let old_data = [0xa5; 64];
    first_packet(control::start_control_read(
        &mut state,
        old_data.as_ptr(),
        old_data.len(),
        255,
    ));
    assert!(state.tx_zlp);
    state.abort_transfer();
    assert_cancelled(&mut state);
    let mut data = first_packet(control::handle_setup(
        &mut state,
        [0x80, 6, 0, 2, 0, 0, 255, 0],
    ));
    loop {
        match control::complete_ep0_in(&mut state, false) {
            control::Ep0InCompletion::SendPacket { data: ptr, len } => {
                assert!(len > 0);
                data.extend_from_slice(unsafe { core::slice::from_raw_parts(ptr, len) });
            }
            control::Ep0InCompletion::ArmOut => break,
            _ => panic!("unexpected completion"),
        }
    }
    let mut expected = [0; descriptors::CONFIG_DESCRIPTOR_LEN];
    descriptors::build_config_descriptor(&CONFIG, &mut expected);
    assert_eq!(data, expected);
}

#[test]
fn cancellation_preserves_configuration_and_reset_allows_reenumeration() {
    let mut state = initialized();
    control::handle_setup(&mut state, [0, 9, 1, 0, 0, 0, 0, 0]);
    first_packet(control::handle_setup(
        &mut state,
        [0x80, 6, 0, 2, 0, 0, 255, 0],
    ));
    state.abort_transfer();
    assert_cancelled(&mut state);
    assert_eq!(state.configuration, 1);
    assert!(matches!(
        control::handle_setup(&mut state, [0, 9, 0, 0, 0, 0, 0, 0]),
        control::SetupAction::ConfigurationChanged(0)
    ));
    assert!(matches!(
        control::complete_ep0_in(&mut state, false),
        control::Ep0InCompletion::ArmOut
    ));
    assert!(matches!(
        control::handle_setup(&mut state, [0, 9, 1, 0, 0, 0, 0, 0]),
        control::SetupAction::ConfigurationChanged(1)
    ));
    reset(&mut state);
    assert_eq!(state.configuration, 0);
    assert_cancelled(&mut state);
    let data = first_packet(control::handle_setup(
        &mut state,
        [0x80, 6, 0, 1, 0, 0, 8, 0],
    ));
    assert_eq!(data.len(), 8);
    assert_eq!(data[0..2], [18, 1]);
}
