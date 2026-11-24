// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

pub mod control;
pub mod descriptors;
pub mod dmx;
pub mod hardware;
pub mod midi;
pub mod queues;

pub use descriptors::UsbMode;
pub use dmx::DmxFrame;
pub use midi::{SysexMessage, UsbMidiPacket, SYSEX_MAX_LEN};
use crate::app::MidiEvent;
use dmx::dmx_queues;
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
    init_with_mode(cfg, UsbMode::Midi);
}

pub fn init_with_mode(cfg: &'static UsbDeviceConfig, mode: UsbMode) {
    hardware::init_with_mode(cfg, mode);
}

pub fn set_mode(mode: UsbMode) {
    hardware::reinit_mode(mode);
}

pub fn current_mode() -> UsbMode {
    cortex_m::interrupt::free(|_| control::control().mode)
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

pub fn take_dmx_frame() -> Option<DmxFrame> {
    cortex_m::interrupt::free(|_| dmx_queues().take_frame())
}

pub fn enqueue_tx_message(port: u8, data: &[u8]) -> Result<(), ()> {
    if current_mode() != UsbMode::Midi {
        return Ok(());
    }
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
