// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use crate::app::{AftertouchEvent, App, AppId, MidiEvent, SurfaceEvent};
use crate::sys::led;
use crate::utils::layout::dr_to_xy;

#[cfg(feature = "launchpad-mini-mk3")]
static BOOT_DATA: &[u8] = include_bytes!("../../../animations/launchpad-mini-mk3.bin");

#[cfg(feature = "launchpad-mk2")]
static BOOT_DATA: &[u8] = include_bytes!("../../../animations/launchpad-mk2.bin");

#[cfg(feature = "launchpad-pro")]
static BOOT_DATA: &[u8] = include_bytes!("../../../animations/launchpad-pro.bin");

#[cfg(feature = "launchpad-pro-mk3")]
static BOOT_DATA: &[u8] = include_bytes!("../../../animations/launchpad-pro-mk3.bin");

// Animation for X and fallback for mini mk1 and s
#[cfg(not(any(
    feature = "launchpad-mini-mk3",
    feature = "launchpad-mk2",
    feature = "launchpad-pro",
    feature = "launchpad-pro-mk3"
)))]
static BOOT_DATA: &[u8] = include_bytes!("../../../animations/launchpad-x.bin");

pub struct BootApp {
    data: &'static [u8],
    tick: u16,
    frame_index: u16,
    offset: usize,
    requested_switch: Option<AppId>,
}

pub type BootAnimationApp = BootApp;

impl BootApp {
    pub const fn new() -> Self {
        Self {
            data: BOOT_DATA,
            tick: 0,
            frame_index: 0,
            offset: 4,
            requested_switch: None,
        }
    }
}

impl App for BootApp {
    fn on_enter(&mut self) {
        self.tick = 0;
        self.frame_index = 0;
        self.offset = 4;
        self.requested_switch = None;
        led::clear();
    }

    fn on_exit(&mut self) {}

    fn on_surface(&mut self, _event: SurfaceEvent) {}

    fn on_midi(&mut self, _event: MidiEvent) {}

    fn on_aftertouch(&mut self, _event: AftertouchEvent) {}

    fn on_tick(&mut self) {
        if self.data.len() < 4 {
            self.requested_switch = Some(crate::app::default_mode());
            return;
        }

        let end_tick = u16::from_le_bytes([self.data[0], self.data[1]]);
        let num_frames = u16::from_le_bytes([self.data[2], self.data[3]]);

        while self.frame_index < num_frames && self.offset + 3 <= self.data.len() {
            let frame_tick = u16::from_le_bytes([self.data[self.offset], self.data[self.offset + 1]]);
            if frame_tick > self.tick {
                break;
            }

            let num_groups = self.data[self.offset + 2] as usize;
            self.offset += 3;

            for _ in 0..num_groups {
                if self.offset + 2 > self.data.len() {
                    break;
                }
                let velocity = self.data[self.offset];
                let count_byte = self.data[self.offset + 1];
                self.offset += 2;

                if (count_byte & 0x80) != 0 {
                    let mask_len = (count_byte & 0x7F) as usize;
                    if self.offset + mask_len > self.data.len() {
                        break;
                    }
                    let mask = &self.data[self.offset..self.offset + mask_len];
                    self.offset += mask_len;

                    for (byte_idx, &b) in mask.iter().enumerate() {
                        if b == 0 {
                            continue;
                        }
                        for bit in 0..8 {
                            if (b & (1 << bit)) != 0 {
                                let led = (byte_idx * 8 + bit) as u8;
                                led::novation(dr_to_xy(led), velocity);
                            }
                        }
                    }
                } else {
                    let count = count_byte as usize;
                    if self.offset + count > self.data.len() {
                        break;
                    }
                    let leds = &self.data[self.offset..self.offset + count];
                    self.offset += count;

                    for &led in leds {
                        led::novation(dr_to_xy(led), velocity);
                    }
                }
            }

            self.frame_index += 1;
        }

        if self.tick >= end_tick {
            self.requested_switch = Some(crate::app::default_mode());
            return;
        }

        self.tick = self.tick.saturating_add(1);
    }

    fn take_requested_app_switch(&mut self) -> Option<AppId> {
        self.requested_switch.take()
    }
}
