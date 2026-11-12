// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use core::cell::UnsafeCell;
use crate::app::MidiEvent;
use super::midi::{SysexMessage, UsbMidiPacket};

pub const MIDI_RX_QUEUE_SIZE: usize = 1025;
pub const SYSEX_RX_QUEUE_SIZE: usize = 5;
pub const MIDI_TX_QUEUE_SIZE: usize = 64;

pub struct Ring<T, const N: usize> {
    buffer: [T; N],
    head: usize,
    tail: usize,
}

impl<T: Copy, const N: usize> Ring<T, N> {
    pub const fn new(init: T) -> Self {
        Self {
            buffer: [init; N],
            head: 0,
            tail: 0,
        }
    }

    #[inline(always)]
    fn next_index(index: usize) -> usize {
        let next = index + 1;
        if next == N {
            0
        } else {
            next
        }
    }

    #[inline(always)]
    pub fn push(&mut self, item: T) -> bool {
        let next = Self::next_index(self.head);
        if next == self.tail {
            return false;
        }
        self.buffer[self.head] = item;
        self.head = next;
        true
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.head == self.tail {
            return None;
        }
        let item = self.buffer[self.tail];
        self.tail = Self::next_index(self.tail);
        Some(item)
    }

    pub fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
    }

    pub fn len(&self) -> usize {
        if self.head >= self.tail {
            self.head - self.tail
        } else {
            N - self.tail + self.head
        }
    }

    pub fn free_space(&self) -> usize {
        (N - 1) - self.len()
    }
}

pub struct UsbQueues {
    pub midi_rx: Ring<MidiEvent, MIDI_RX_QUEUE_SIZE>,
    pub sysex_rx: Ring<SysexMessage, SYSEX_RX_QUEUE_SIZE>,
    pub midi_tx: Ring<UsbMidiPacket, MIDI_TX_QUEUE_SIZE>,
}

impl UsbQueues {
    pub fn clear(&mut self) {
        self.midi_rx.clear();
        self.sysex_rx.clear();
        self.midi_tx.clear();
    }

    pub const fn new() -> Self {
        Self {
            midi_rx: Ring::new(MidiEvent {
                port: crate::app::MidiPort::Daw,
                status: 0,
                data1: 0,
                data2: 0,
            }),
            sysex_rx: Ring::new(SysexMessage::empty()),
            midi_tx: Ring::new(UsbMidiPacket::empty()),
        }
    }
}

pub struct UsbQueuesCell(UnsafeCell<UsbQueues>);
unsafe impl Sync for UsbQueuesCell {}

pub static QUEUES: UsbQueuesCell = UsbQueuesCell(UnsafeCell::new(UsbQueues::new()));

#[inline(always)]
pub fn queues() -> &'static mut UsbQueues {
    unsafe { &mut *QUEUES.0.get() }
}
