// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 ZephyrCodesStuff

//! Enttec DMX USB Pro wire protocol parser and reply generator.
//!
//! Frame format:
//! `[0x7E, label, len_lsb, len_msb, data[0..len], 0xE7]`
//!
//! DMX512 channel mapping:
//! 64 RGB pads (8x8 main grid), row-major top-to-bottom, left-to-right
//! (channels 1..=192 -> Launchpad XY indices 81..=88, 71..=78, ..., 11..=18).

pub const DMX_LED_COUNT: usize = 64;
pub const DMX_RGB_CHANNELS: usize = DMX_LED_COUNT * 3;
pub const DMX_MAX_PAYLOAD_LEN: usize = 600;

/// Two-byte FTDI FT232R modem/line status prefix prepended to every Bulk IN packet.
/// Byte 0: 0x01 (reserved bit set, no modem status lines asserted).
/// Byte 1: 0x60 (THRE | TEMT — transmitter holding/shift registers empty).
pub const FTDI_MODEM_STATUS: [u8; 2] = [0x01, 0x60];

pub const ENTTEC_START_DELIM: u8 = 0x7E;
pub const ENTTEC_END_DELIM: u8 = 0xE7;

pub const LABEL_GET_WIDGET_PARAMS: u8 = 3;
pub const LABEL_SET_WIDGET_PARAMS: u8 = 4;
pub const LABEL_OUTPUT_ONLY_SEND_DMX: u8 = 6;
pub const LABEL_RECEIVE_DMX_ON_CHANGE: u8 = 8;
pub const LABEL_GET_WIDGET_SERIAL: u8 = 10;

/// Default Enttec DMX USB Pro firmware version (1.44).
pub const WIDGET_FW_LSB: u8 = 44;
pub const WIDGET_FW_MSB: u8 = 1;

/// Default 4-byte BCD widget serial number (matches USB serial "EN000001").
pub const WIDGET_SERIAL: [u8; 4] = [0x01, 0x00, 0x00, 0x00];

/// Captures the first 193 bytes of payload (DMX start code + 192 RGB channel bytes).
/// Channels beyond 192 are skipped while still validating packet framing.
const CAPTURE_BUF_LEN: usize = 1 + DMX_RGB_CHANNELS;

#[derive(Copy, Clone)]
pub struct DmxFrame {
    pub channels: [u8; DMX_RGB_CHANNELS],
}

impl DmxFrame {
    pub const fn empty() -> Self {
        Self {
            channels: [0; DMX_RGB_CHANNELS],
        }
    }
}

/// Maps pad index `0..64` (row-major, top-to-bottom, left-to-right) to
/// Launchpad canonical XY coordinate (`81..=88` for top row down to `11..=18` for bottom row).
#[inline]
pub const fn pad_index_to_xy(pad: usize) -> u8 {
    let row_from_top = pad / 8;
    let col = pad % 8;
    ((8 - row_from_top) * 10 + (col + 1)) as u8
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum ParseState {
    WaitStart,
    ReadLabel,
    ReadLenLsb,
    ReadLenMsb,
    ReadPayload,
    WaitEnd,
}

pub struct DmxParser {
    state: ParseState,
    label: u8,
    expected_len: usize,
    received_len: usize,
    buf: [u8; CAPTURE_BUF_LEN],
    pub break_time: u8,
    pub mab_time: u8,
    pub refresh_rate: u8,
}

impl DmxParser {
    pub const fn new() -> Self {
        Self {
            state: ParseState::WaitStart,
            label: 0,
            expected_len: 0,
            received_len: 0,
            buf: [0; CAPTURE_BUF_LEN],
            break_time: 9,
            mab_time: 1,
            refresh_rate: 40,
        }
    }

    pub fn reset(&mut self) {
        self.state = ParseState::WaitStart;
        self.label = 0;
        self.expected_len = 0;
        self.received_len = 0;
    }

    pub fn parse_into_queues(&mut self, bytes: &[u8], queues: &mut DmxQueues) {
        let mut latched: Option<DmxFrame> = None;
        self.parse_bytes(
            bytes,
            &mut |frame| latched = Some(*frame),
            &mut |byte| queues.push_tx(byte),
        );
        if let Some(frame) = latched {
            queues.latch_frame(&frame);
        }
    }

    pub fn parse_bytes(
        &mut self,
        bytes: &[u8],
        on_frame: &mut impl FnMut(&DmxFrame),
        on_tx_byte: &mut impl FnMut(u8) -> bool,
    ) {
        for &byte in bytes {
            self.feed_byte(byte, on_frame, on_tx_byte);
        }
    }

    fn feed_byte(
        &mut self,
        byte: u8,
        on_frame: &mut impl FnMut(&DmxFrame),
        on_tx_byte: &mut impl FnMut(u8) -> bool,
    ) {
        match self.state {
            ParseState::WaitStart => {
                if byte == ENTTEC_START_DELIM {
                    self.state = ParseState::ReadLabel;
                }
            }
            ParseState::ReadLabel => {
                self.label = byte;
                self.state = ParseState::ReadLenLsb;
            }
            ParseState::ReadLenLsb => {
                self.expected_len = byte as usize;
                self.state = ParseState::ReadLenMsb;
            }
            ParseState::ReadLenMsb => {
                self.expected_len |= (byte as usize) << 8;
                self.received_len = 0;
                if self.expected_len > DMX_MAX_PAYLOAD_LEN {
                    self.state = if byte == ENTTEC_START_DELIM {
                        ParseState::ReadLabel
                    } else {
                        ParseState::WaitStart
                    };
                } else if self.expected_len == 0 {
                    self.state = ParseState::WaitEnd;
                } else {
                    self.state = ParseState::ReadPayload;
                }
            }
            ParseState::ReadPayload => {
                if self.received_len < CAPTURE_BUF_LEN {
                    self.buf[self.received_len] = byte;
                }
                self.received_len += 1;
                if self.received_len >= self.expected_len {
                    self.state = ParseState::WaitEnd;
                }
            }
            ParseState::WaitEnd => {
                if byte == ENTTEC_END_DELIM {
                    self.dispatch(on_frame, on_tx_byte);
                    self.state = ParseState::WaitStart;
                } else if byte == ENTTEC_START_DELIM {
                    self.state = ParseState::ReadLabel;
                } else {
                    self.state = ParseState::WaitStart;
                }
            }
        }
    }

    fn dispatch(
        &mut self,
        on_frame: &mut impl FnMut(&DmxFrame),
        on_tx_byte: &mut impl FnMut(u8) -> bool,
    ) {
        match self.label {
            LABEL_GET_WIDGET_PARAMS => {
                let payload = [
                    WIDGET_FW_LSB,
                    WIDGET_FW_MSB,
                    self.break_time,
                    self.mab_time,
                    self.refresh_rate,
                ];
                send_enttec_packet(LABEL_GET_WIDGET_PARAMS, &payload, on_tx_byte);
            }
            LABEL_SET_WIDGET_PARAMS => {
                if self.expected_len >= 5 {
                    self.break_time = self.buf[2];
                    self.mab_time = self.buf[3];
                    self.refresh_rate = self.buf[4];
                }
            }
            LABEL_OUTPUT_ONLY_SEND_DMX => {
                if self.expected_len >= 1 && self.buf[0] == 0x00 {
                    let mut frame = DmxFrame::empty();
                    let channel_count = (self.expected_len - 1).min(DMX_RGB_CHANNELS);
                    frame.channels[..channel_count]
                        .copy_from_slice(&self.buf[1..1 + channel_count]);
                    on_frame(&frame);
                }
            }
            LABEL_GET_WIDGET_SERIAL => {
                send_enttec_packet(LABEL_GET_WIDGET_SERIAL, &WIDGET_SERIAL, on_tx_byte);
            }
            LABEL_RECEIVE_DMX_ON_CHANGE => {}
            _ => {}
        }
    }
}

pub fn send_enttec_packet(label: u8, payload: &[u8], on_tx_byte: &mut impl FnMut(u8) -> bool) {
    let len = payload.len();
    let _ = on_tx_byte(ENTTEC_START_DELIM);
    let _ = on_tx_byte(label);
    let _ = on_tx_byte((len & 0xff) as u8);
    let _ = on_tx_byte(((len >> 8) & 0xff) as u8);
    for &b in payload {
        let _ = on_tx_byte(b);
    }
    let _ = on_tx_byte(ENTTEC_END_DELIM);
}

pub const DMX_TX_QUEUE_SIZE: usize = 64;

pub struct DmxQueues {
    pub frame: DmxFrame,
    pub dirty: bool,
    tx_buf: [u8; DMX_TX_QUEUE_SIZE],
    tx_head: usize,
    tx_tail: usize,
}

impl DmxQueues {
    pub const fn new() -> Self {
        Self {
            frame: DmxFrame::empty(),
            dirty: false,
            tx_buf: [0; DMX_TX_QUEUE_SIZE],
            tx_head: 0,
            tx_tail: 0,
        }
    }

    pub fn clear(&mut self) {
        self.frame = DmxFrame::empty();
        self.dirty = false;
        self.tx_head = 0;
        self.tx_tail = 0;
    }

    pub fn latch_frame(&mut self, frame: &DmxFrame) {
        self.frame = *frame;
        self.dirty = true;
    }

    pub fn take_frame(&mut self) -> Option<DmxFrame> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        Some(self.frame)
    }

    pub fn push_tx(&mut self, byte: u8) -> bool {
        let next = (self.tx_head + 1) % DMX_TX_QUEUE_SIZE;
        if next == self.tx_tail {
            return false;
        }
        self.tx_buf[self.tx_head] = byte;
        self.tx_head = next;
        true
    }

    pub fn pop_tx(&mut self) -> Option<u8> {
        if self.tx_head == self.tx_tail {
            return None;
        }
        let byte = self.tx_buf[self.tx_tail];
        self.tx_tail = (self.tx_tail + 1) % DMX_TX_QUEUE_SIZE;
        Some(byte)
    }

    pub fn tx_is_empty(&self) -> bool {
        self.tx_head == self.tx_tail
    }

    /// Builds one FTDI FT232R Bulk IN packet (2-byte modem status header + up to 62 TX bytes).
    /// Returns `0` if no TX bytes are queued.
    pub fn pop_ftdi_in_packet(&mut self, out: &mut [u8; 64]) -> usize {
        if self.tx_is_empty() {
            return 0;
        }
        out[0] = FTDI_MODEM_STATUS[0];
        out[1] = FTDI_MODEM_STATUS[1];
        let mut count = 2;
        while count < out.len() {
            let Some(byte) = self.pop_tx() else {
                break;
            };
            out[count] = byte;
            count += 1;
        }
        count
    }
}

struct DmxQueuesCell(core::cell::UnsafeCell<DmxQueues>);
unsafe impl Sync for DmxQueuesCell {}

static DMX_QUEUES: DmxQueuesCell = DmxQueuesCell(core::cell::UnsafeCell::new(DmxQueues::new()));

#[inline(always)]
pub fn dmx_queues() -> &'static mut DmxQueues {
    unsafe { &mut *DMX_QUEUES.0.get() }
}
