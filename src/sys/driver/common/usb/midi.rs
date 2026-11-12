// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use crate::app::{MidiEvent, MidiPort};

pub const SYSEX_MAX_LEN: usize = 600;
pub const MIDI_TX_MAX_PACKET_COUNT: usize = 32;

#[derive(Copy, Clone)]
#[repr(C)]
pub struct UsbMidiPacket {
    pub data: [u8; 4],
}

impl UsbMidiPacket {
    pub const fn empty() -> Self {
        Self { data: [0; 4] }
    }
}

#[derive(Copy, Clone)]
pub struct SysexMessage {
    pub port: MidiPort,
    pub len: usize,
    pub data: [u8; SYSEX_MAX_LEN],
}

impl SysexMessage {
    pub const fn empty() -> Self {
        Self {
            port: MidiPort::Daw,
            len: 0,
            data: [0; SYSEX_MAX_LEN],
        }
    }
}

pub struct SysexReceiver {
    pub buf: [u8; SYSEX_MAX_LEN],
    pub len: usize,
    pub port: MidiPort,
}

impl SysexReceiver {
    pub const fn new() -> Self {
        Self {
            buf: [0; SYSEX_MAX_LEN],
            len: 0,
            port: MidiPort::Daw,
        }
    }

    pub fn append(&mut self, bytes: &[u8]) {
        if self.len == 0 && bytes.first() != Some(&0xf0) {
            return;
        }
        let space = SYSEX_MAX_LEN.saturating_sub(self.len);
        let to_copy = bytes.len().min(space);
        self.buf[self.len..self.len + to_copy].copy_from_slice(&bytes[..to_copy]);
        self.len += to_copy;
    }

    pub fn reset(&mut self) {
        self.len = 0;
        self.port = MidiPort::Daw;
    }

    pub fn take(&mut self) -> Option<SysexMessage> {
        if self.len == 0 {
            return None;
        }

        let mut data = [0u8; SYSEX_MAX_LEN];
        data[..self.len].copy_from_slice(&self.buf[..self.len]);
        let msg = SysexMessage {
            port: self.port,
            len: self.len,
            data,
        };
        self.len = 0;
        Some(msg)
    }
}

pub fn parse_usb_midi_packet(
    packet: &[u8],
    sysex_rx: &mut SysexReceiver,
    push_midi: &mut impl FnMut(MidiEvent) -> bool,
    push_sysex: &mut impl FnMut(SysexMessage) -> bool,
) {
    if packet.len() < 4 {
        return;
    }

    let cin = packet[0] & 0x0f;
    let cable = packet[0] >> 4;
    let port = match cable {
        0 => MidiPort::Daw,
        1 => MidiPort::Midi,
        _ => MidiPort::Din,
    };

    match cin {
        0x4 => {
            sysex_rx.port = port;
            sysex_rx.append(&packet[1..4]);
        }
        0x5 => {
            sysex_rx.port = port;
            sysex_rx.append(&packet[1..2]);
            if let Some(msg) = sysex_rx.take() {
                let _ = push_sysex(msg);
            }
        }
        0x6 => {
            sysex_rx.port = port;
            sysex_rx.append(&packet[1..3]);
            if let Some(msg) = sysex_rx.take() {
                let _ = push_sysex(msg);
            }
        }
        0x7 => {
            sysex_rx.port = port;
            sysex_rx.append(&packet[1..4]);
            if let Some(msg) = sysex_rx.take() {
                let _ = push_sysex(msg);
            }
        }
        0x8 | 0x9 | 0xA | 0xB | 0xC | 0xD | 0xE => {
            let event = MidiEvent {
                port,
                status: packet[1],
                data1: packet[2],
                data2: packet[3],
            };
            let _ = push_midi(event);
        }
        0xF => {
            if packet[1] != 0 {
                let event = MidiEvent {
                    port,
                    status: packet[1],
                    data1: 0,
                    data2: 0,
                };
                let _ = push_midi(event);
            }
        }
        _ => {}
    }
}

pub fn encode_usb_midi_packets(
    port: u8,
    data: &[u8],
    out: &mut [UsbMidiPacket],
) -> Result<usize, ()> {
    if data.is_empty() {
        return Err(());
    }

    let cable = (port & 0x0f) << 4;

    if data[0] == 0xf0 {
        return encode_sysex_packets(cable, data, out);
    }

    let Some((cin, message_len)) = short_message_format(data[0]) else {
        return Err(());
    };

    if data.len() < message_len || out.is_empty() {
        return Err(());
    }

    out[0] = UsbMidiPacket {
        data: [
            cable | cin,
            data[0],
            if message_len > 1 { data[1] } else { 0 },
            if message_len > 2 { data[2] } else { 0 },
        ],
    };
    Ok(1)
}

fn encode_sysex_packets(
    cable: u8,
    data: &[u8],
    out: &mut [UsbMidiPacket],
) -> Result<usize, ()> {
    let mut src = 0usize;
    let mut dst = 0usize;

    while src < data.len() {
        if dst >= out.len() {
            return Err(());
        }

        let remain = data.len() - src;
        let take = remain.min(3);
        let cin = if remain > 3 {
            0x4
        } else {
            match remain {
                1 => 0x5,
                2 => 0x6,
                3 => 0x7,
                _ => return Err(()),
            }
        };

        out[dst] = UsbMidiPacket {
            data: [
                cable | cin,
                data[src],
                if take > 1 { data[src + 1] } else { 0 },
                if take > 2 { data[src + 2] } else { 0 },
            ],
        };
        src += take;
        dst += 1;
    }

    Ok(dst)
}

fn short_message_format(status: u8) -> Option<(u8, usize)> {
    match status {
        0x80..=0x8f => Some((0x8, 3)),
        0x90..=0x9f => Some((0x9, 3)),
        0xa0..=0xaf => Some((0xa, 3)),
        0xb0..=0xbf => Some((0xb, 3)),
        0xc0..=0xcf => Some((0xc, 2)),
        0xd0..=0xdf => Some((0xd, 2)),
        0xe0..=0xef => Some((0xe, 3)),
        0xf1 | 0xf3 => Some((0x2, 2)),
        0xf2 => Some((0x3, 3)),
        0xf6 => Some((0x5, 1)),
        0xf8..=0xff => Some((0xf, 1)),
        _ => None,
    }
}
