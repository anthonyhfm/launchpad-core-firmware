// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister

#![allow(dead_code)]

#[path = "../src/app/events.rs"]
pub mod events;
#[path = "../src/sys/midi.rs"]
pub mod transport;
mod app {
    pub use crate::events::*;
    pub use crate::transport::MidiPort;
}
mod sys {
    pub use crate::transport as midi;
}
#[path = "../src/sys/driver/common/usb/control.rs"]
mod control;
#[path = "../src/sys/driver/common/usb/descriptors.rs"]
mod descriptors;
#[path = "../src/sys/driver/common/usb/midi.rs"]
mod midi;
#[path = "../src/sys/driver/common/usb/queues.rs"]
mod queues;

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
    product: "USB regression",
    serial_number: "test",
    port1_name: "DAW",
    port2_name: "MIDI",
    use_ep2_for_out: false,
};
static CONFIG_EP0_8: UsbDeviceConfig = UsbDeviceConfig {
    ep0_max_packet_size: 8,
    ..CONFIG
};

fn assert_idle(state: &mut control::ControlState) {
    assert_eq!(state.pending_address, None);
    assert!(state.tx_data.is_null());
    assert_eq!(state.tx_len, 0);
    assert_eq!(state.tx_pos, 0);
    assert!(!state.tx_zlp);
    assert!(control::next_ep0_chunk(state).is_none());
}

#[test]
fn set_address_zero_and_127_remain_pending_until_status_completion() {
    let mut state = control::ControlState::new();
    assert_idle(&mut state);
    state.init(&CONFIG);
    for address in [0, 127] {
        assert!(matches!(
            control::handle_setup(&mut state, [0, 5, address, 0, 0, 0, 0, 0]),
            control::SetupAction::StatusIn
        ));
        assert_eq!(state.pending_address, Some(address));
        assert_eq!(state.pending_address.take(), Some(address));
        assert_eq!(state.pending_address.take(), None);
        assert_idle(&mut state);
    }
}

#[test]
fn set_address_rejects_128_and_malformed_fields() {
    let mut state = control::ControlState::new();
    state.init(&CONFIG);
    for setup in [
        [0, 5, 128, 0, 0, 0, 0, 0],
        [0, 5, 0, 1, 0, 0, 0, 0],
        [1, 5, 42, 0, 0, 0, 0, 0],
        [2, 5, 42, 0, 0, 0, 0, 0],
        [0x80, 5, 42, 0, 0, 0, 0, 0],
        [0x20, 5, 42, 0, 0, 0, 0, 0],
        [0x40, 5, 42, 0, 0, 0, 0, 0],
        [0, 5, 42, 0, 1, 0, 0, 0],
        [0, 5, 42, 0, 0, 1, 0, 0],
        [0, 5, 42, 0, 0, 0, 1, 0],
        [0, 5, 42, 0, 0, 0, 0, 1],
    ] {
        state.pending_address = Some(17);
        assert!(
            matches!(
                control::handle_setup(&mut state, setup),
                control::SetupAction::Stall
            ),
            "accepted malformed SET_ADDRESS: {setup:?}"
        );
        assert_idle(&mut state);
    }
    assert!(matches!(
        control::handle_setup(&mut state, [0, 9, 1, 0, 0, 0, 0, 0]),
        control::SetupAction::ConfigurationChanged(1)
    ));
    assert!(matches!(
        control::handle_setup(&mut state, [0, 5, 42, 0, 0, 0, 0, 0]),
        control::SetupAction::Stall
    ));
    assert_idle(&mut state);
}

#[test]
fn every_setup_discards_old_address_and_transfer_including_zlp() {
    let mut state = control::ControlState::new();
    state.init(&CONFIG);
    for setup in [
        [0, 1, 0, 0, 0, 0, 0, 0],
        [0, 0xff, 0, 0, 0, 0, 0, 0],
        [0x40, 1, 0, 0, 0, 0, 0, 0],
    ] {
        let data = state.config_desc.as_ptr();
        control::start_control_read(&mut state, data, 64, 128);
        assert!(state.tx_zlp);
        state.pending_address = Some(42);
        control::handle_setup(&mut state, setup);
        assert_idle(&mut state);
    }

    control::handle_setup(&mut state, [0x80, 6, 0, 2, 0, 0, 255, 0]);
    state.pending_address = Some(127);
    assert!(matches!(
        control::handle_setup(&mut state, [0x80, 8, 0, 0, 0, 0, 1, 0]),
        control::SetupAction::SendPacket { len: 1, .. }
    ));
    assert_eq!(state.pending_address, None);
    assert_eq!(state.tx_len, 1);
    assert!(control::next_ep0_chunk(&mut state).is_none());

    state.cfg = None;
    state.pending_address = Some(0);
    state.tx_zlp = true;
    assert!(matches!(
        control::handle_setup(&mut state, [0; 8]),
        control::SetupAction::Stall
    ));
    assert_idle(&mut state);
}

#[test]
fn enumeration_restarts_after_reset_with_runtime_descriptors() {
    for cfg in [&CONFIG, &CONFIG_EP0_8] {
        let mut state = control::ControlState::new();
        state.init(cfg);
        control::handle_setup(&mut state, [0, 5, 42, 0, 0, 0, 0, 0]);
        state.init(cfg);
        assert_idle(&mut state);
        control::handle_setup(&mut state, [0, 9, 1, 0, 0, 0, 0, 0]);
        control::handle_setup(&mut state, [0x80, 6, 0, 2, 0, 0, 255, 0]);
        assert!(state.tx_pos < state.tx_len);
        state.init(cfg);
        assert_eq!(state.configuration, 0);
        assert_idle(&mut state);

        let control::SetupAction::SendPacket { data, len } =
            control::handle_setup(&mut state, [0x80, 6, 0, 2, 0, 0, 255, 0])
        else {
            panic!("configuration descriptor unavailable after reset")
        };
        assert_eq!(data, state.config_desc.as_ptr());
        assert_eq!(len, cfg.ep0_max_packet_size as usize);
        let mut received = unsafe { core::slice::from_raw_parts(data, len) }.to_vec();
        while let Some((data, len)) = control::next_ep0_chunk(&mut state) {
            assert!(len <= cfg.ep0_max_packet_size as usize);
            received.extend_from_slice(unsafe { core::slice::from_raw_parts(data, len) });
        }
        assert_eq!(received.as_slice(), state.config_desc.as_slice());
        assert_eq!(received.len(), descriptors::CONFIG_DESCRIPTOR_LEN);
        assert!(matches!(
            control::handle_setup(&mut state, [0, 5, 0, 0, 0, 0, 0, 0]),
            control::SetupAction::StatusIn
        ));
        assert_eq!(state.pending_address.take(), Some(0));
        assert!(matches!(
            control::handle_setup(&mut state, [0, 9, 1, 0, 0, 0, 0, 0]),
            control::SetupAction::ConfigurationChanged(1)
        ));
    }
}

#[test]
fn full_midi_queues_clear_and_accept_only_the_new_session() {
    let mut q = queues::UsbQueues::new();
    let mut event = app::MidiEvent {
        port: transport::MidiPort::Daw,
        status: 0x90,
        data1: 60,
        data2: 127,
    };
    for _ in 0..2 {
        while q.midi_rx.push(event) {}
        while q.sysex_rx.push(midi::SysexMessage::empty()) {}
        while q.midi_tx.push(midi::UsbMidiPacket::empty()) {}
        assert_eq!(q.midi_rx.free_space(), 0);
        assert_eq!(q.sysex_rx.free_space(), 0);
        assert_eq!(q.midi_tx.free_space(), 0);
        q.clear();
        q.clear();
        assert!(q.midi_rx.pop().is_none());
        assert!(q.sysex_rx.pop().is_none());
        assert!(q.midi_tx.pop().is_none());
        assert_eq!(q.midi_rx.free_space(), queues::MIDI_RX_QUEUE_SIZE - 1);
        assert_eq!(q.sysex_rx.free_space(), queues::SYSEX_RX_QUEUE_SIZE - 1);
        assert_eq!(q.midi_tx.free_space(), queues::MIDI_TX_QUEUE_SIZE - 1);

        event.data1 = 61;
        let mut message = midi::SysexMessage::empty();
        message.len = 3;
        message.data[..3].copy_from_slice(&[0xf0, 0x7d, 0xf7]);
        let packet = midi::UsbMidiPacket {
            data: [9, 0x90, 61, 127],
        };
        for _ in 0..queues::MIDI_RX_QUEUE_SIZE + 1 {
            assert!(q.midi_rx.push(event));
            assert_eq!(q.midi_rx.pop().unwrap().data1, 61);
            assert!(q.sysex_rx.push(message));
            let received = q.sysex_rx.pop().unwrap();
            assert_eq!(&received.data[..received.len], &[0xf0, 0x7d, 0xf7]);
            assert!(q.midi_tx.push(packet));
            assert_eq!(q.midi_tx.pop().unwrap().data, packet.data);
        }
    }
}

#[test]
fn interrupted_sysex_cannot_cross_a_usb_reset() {
    let mut receiver = midi::SysexReceiver::new();
    let mut q = queues::UsbQueues::new();
    let mut messages = Vec::new();
    midi::parse_usb_midi_packet(
        &[0x14, 0xf0, 0x01, 0x02],
        &mut receiver,
        &mut |_| true,
        &mut |msg| {
            messages.push(msg);
            true
        },
    );
    assert!(q.midi_rx.push(app::MidiEvent {
        port: transport::MidiPort::Midi,
        status: 0x90,
        data1: 60,
        data2: 127,
    }));
    assert!(q.sysex_rx.push(midi::SysexMessage::empty()));
    assert!(q.midi_tx.push(midi::UsbMidiPacket::empty()));
    q.clear();
    receiver.reset();
    assert!(q.midi_rx.pop().is_none());
    assert!(q.sysex_rx.pop().is_none());
    assert!(q.midi_tx.pop().is_none());
    midi::parse_usb_midi_packet(
        &[0x16, 0x03, 0xf7, 0],
        &mut receiver,
        &mut |_| true,
        &mut |msg| {
            messages.push(msg);
            true
        },
    );
    assert!(messages.is_empty());
    midi::parse_usb_midi_packet(
        &[0x07, 0xf0, 0x7d, 0xf7],
        &mut receiver,
        &mut |_| true,
        &mut |msg| {
            messages.push(msg);
            true
        },
    );
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].port, transport::MidiPort::Daw);
    assert_eq!(&messages[0].data[..messages[0].len], &[0xf0, 0x7d, 0xf7]);
}

#[test]
fn reset_discards_partial_sysex_on_every_cable() {
    let mut receiver = midi::SysexReceiver::new();
    let mut messages = Vec::new();
    for cable in 0u8..3 {
        midi::parse_usb_midi_packet(
            &[cable << 4 | 0x04, 0xf0, 0x7d, cable],
            &mut receiver,
            &mut |_| true,
            &mut |message| {
                messages.push(message);
                true
            },
        );
    }
    receiver.reset();
    for cable in 0u8..3 {
        midi::parse_usb_midi_packet(
            &[cable << 4 | 0x06, 0x11, 0xf7, 0],
            &mut receiver,
            &mut |_| true,
            &mut |message| {
                messages.push(message);
                true
            },
        );
    }
    assert!(messages.is_empty());
    for cable in 0u8..3 {
        midi::parse_usb_midi_packet(
            &[cable << 4 | 0x04, 0xf0, 0x7d, cable],
            &mut receiver,
            &mut |_| true,
            &mut |message| {
                messages.push(message);
                true
            },
        );
    }
    for cable in 0u8..3 {
        midi::parse_usb_midi_packet(
            &[cable << 4 | 0x05, 0xf7, 0, 0],
            &mut receiver,
            &mut |_| true,
            &mut |message| {
                messages.push(message);
                true
            },
        );
    }
    assert_eq!(messages.len(), 3);
    for (index, port) in [
        transport::MidiPort::Daw,
        transport::MidiPort::Midi,
        transport::MidiPort::Din,
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(messages[index].port, port);
        assert_eq!(
            &messages[index].data[..messages[index].len],
            &[0xf0, 0x7d, index as u8, 0xf7]
        );
    }
}
