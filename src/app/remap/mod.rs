// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister
// Remap mode, added for the Launchpad Mini mk1.

//! Remap mode: every button sends what the user-defined table in
//! [`crate::sys::remap`] says (note, CC, program change, bank switch, or the
//! native programmer-mode note). LEDs show each entry's on/off colour, and the
//! host can drive them by sending the mapped note/CC back on the MIDI port.

use crate::app::apptrait::App;
use crate::app::events::{AftertouchEvent, MidiEvent, SurfaceEvent};
use crate::driver;
use crate::sys::led;
use crate::sys::midi::MidiPort;
use crate::sys::remap::{
    self, Entry, KIND_BANK, KIND_CC, KIND_NATIVE, KIND_NONE, KIND_NOTE, KIND_PROGRAM, SLOTS,
};

/// `held[slot]` is 0 when not held, otherwise 1 + the bank active at press
/// time. Zero init keeps the app (and AppHost) in .bss instead of .data.
const NOT_HELD: u8 = 0;

pub struct RemapApp {
    held_bank: [u8; SLOTS],
    drawn_generation: u16,
}

impl RemapApp {
    pub const fn new() -> Self {
        Self {
            held_bank: [NOT_HELD; SLOTS],
            drawn_generation: 0,
        }
    }

    fn send_on(index: u8, e: &Entry) {
        let ch = e.channel & 0x0f;
        match e.kind {
            KIND_NOTE => {
                let velocity = if e.on_value == 0 { 127 } else { e.on_value };
                driver::send_midi(MidiPort::Midi, &[0x90 | ch, e.number, velocity]);
            }
            KIND_CC => driver::send_midi(MidiPort::Midi, &[0xb0 | ch, e.number, e.on_value]),
            KIND_PROGRAM => driver::send_midi(MidiPort::Midi, &[0xc0 | ch, e.number]),
            KIND_NATIVE => driver::send_midi(MidiPort::Midi, &[0x90 | ch, index, 127]),
            _ => {}
        }
    }

    fn send_off(index: u8, e: &Entry) {
        let ch = e.channel & 0x0f;
        match e.kind {
            KIND_NOTE => driver::send_midi(MidiPort::Midi, &[0x80 | ch, e.number, e.off_value]),
            KIND_CC => driver::send_midi(MidiPort::Midi, &[0xb0 | ch, e.number, e.off_value]),
            KIND_NATIVE => driver::send_midi(MidiPort::Midi, &[0x80 | ch, index, 0]),
            _ => {}
        }
    }

    fn draw_slot(index: u8) {
        let color = remap::with(|s| {
            let bank = s.active;
            let e = s.entry(bank, index);
            let on = match e.kind {
                KIND_NONE => return None,
                KIND_BANK => e.number == bank,
                _ => s.is_lit(bank, index),
            };
            Some(if on { e.color_on } else { e.color_off })
        });
        let color = color.unwrap_or(0);
        // bits 0-1 red, bits 2-3 green, each 0..3 -> 6-bit LED level 0..63
        let r = (color & 0x03) * 21;
        let g = ((color >> 2) & 0x03) * 21;
        led::set_rgb(index, r, g, 0);
    }

    fn draw_all(&mut self) {
        for index in 1..SLOTS as u8 {
            Self::draw_slot(index);
        }
        self.drawn_generation = remap::with(|s| s.generation);
    }
}

impl App for RemapApp {
    fn on_enter(&mut self) {
        led::clear();
        self.held_bank = [NOT_HELD; SLOTS];
        self.draw_all();
    }

    fn on_exit(&mut self) {
        // Release held buttons to prevent stuck notes on the host.
        for index in 0..SLOTS {
            let held = self.held_bank[index];
            if held != NOT_HELD {
                let e = remap::with(|s| s.entry(held - 1, index as u8));
                if !e.is_toggle() {
                    Self::send_off(index as u8, &e);
                }
                self.held_bank[index] = NOT_HELD;
            }
        }
    }

    fn on_surface(&mut self, event: SurfaceEvent) {
        let index = event.index;
        if index as usize >= SLOTS {
            return;
        }

        if event.pressed {
            let (bank, e) = remap::with(|s| (s.active, s.entry(s.active, index)));
            self.held_bank[index as usize] = bank + 1;
            match e.kind {
                KIND_NONE => return,
                KIND_BANK => {
                    remap::with(|s| s.select_bank(e.number));
                    return; // on_tick redraws on the generation change
                }
                _ => {}
            }
            if e.is_toggle() {
                let now_on = remap::with(|s| {
                    let on = !s.is_lit(bank, index);
                    s.set_lit(bank, index, on);
                    on
                });
                if now_on {
                    Self::send_on(index, &e);
                } else {
                    Self::send_off(index, &e);
                }
            } else {
                remap::with(|s| s.set_lit(bank, index, true));
                Self::send_on(index, &e);
            }
            Self::draw_slot(index);
        } else {
            // Release with the bank captured at press time: switching banks
            // while holding a pad still sends the matching note off.
            let held = self.held_bank[index as usize];
            if held == NOT_HELD {
                return;
            }
            let bank = held - 1;
            self.held_bank[index as usize] = NOT_HELD;
            let e = remap::with(|s| s.entry(bank, index));
            if e.kind == KIND_NONE || e.kind == KIND_BANK || e.is_toggle() {
                return;
            }
            remap::with(|s| s.set_lit(bank, index, false));
            Self::send_off(index, &e);
            Self::draw_slot(index);
        }
    }

    fn on_midi(&mut self, event: MidiEvent) {
        if event.port != MidiPort::Midi {
            return;
        }
        let ch = event.status & 0x0f;
        let (kind, on) = match event.status & 0xf0 {
            0x90 => (KIND_NOTE, event.data2 > 0),
            0x80 => (KIND_NOTE, false),
            0xb0 => (KIND_CC, true),
            _ => return,
        };
        let changed = remap::with(|s| {
            let mut changed = false;
            for bank in 0..remap::BANKS as u8 {
                for slot in 0..SLOTS as u8 {
                    let e = s.entry(bank, slot);
                    if e.kind != kind || e.channel != ch || e.number != event.data1 {
                        continue;
                    }
                    let lit = if kind == KIND_CC { event.data2 != e.off_value } else { on };
                    if s.is_lit(bank, slot) != lit {
                        s.set_lit(bank, slot, lit);
                        changed = true;
                    }
                }
            }
            changed
        });
        if changed {
            self.draw_all();
        }
    }

    fn on_aftertouch(&mut self, _event: AftertouchEvent) {}

    fn on_tick(&mut self) {
        if remap::with(|s| s.generation) != self.drawn_generation {
            self.draw_all();
        }
    }
}
