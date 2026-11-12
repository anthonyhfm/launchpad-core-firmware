// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

pub mod control;
pub mod descriptors;
pub mod hardware;
pub mod midi;
pub mod queues;

pub use midi::{SysexMessage, UsbMidiPacket, SYSEX_MAX_LEN};
use crate::app::MidiEvent;
use queues::queues;

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

pub fn init_event_queues() {
    // Queues are statically initialized.
}

pub fn init(cfg: &'static UsbDeviceConfig) {
    hardware::init(cfg);
}

pub fn poll() {
    hardware::poll();
}

pub fn dequeue_midi_event() -> Option<MidiEvent> {
    cortex_m::interrupt::free(|_| queues().midi_rx.pop())
}

pub fn dequeue_sysex_message() -> Option<SysexMessage> {
    cortex_m::interrupt::free(|_| queues().sysex_rx.pop())
}

pub fn enqueue_tx_message(port: u8, data: &[u8]) -> Result<(), ()> {
    let mut temp = [UsbMidiPacket::empty(); midi::MIDI_TX_MAX_PACKET_COUNT];
    let count = midi::encode_usb_midi_packets(port, data, &mut temp)?;

    cortex_m::interrupt::free(|_| {
        {
            let q = queues();
            if q.midi_tx.free_space() < count {
                return Err(());
            }

            for packet in temp.iter().take(count) {
                if !q.midi_tx.push(*packet) {
                    return Err(());
                }
            }
        }

        hardware::pump_tx();
        Ok(())
    })
}
