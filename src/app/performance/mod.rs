// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use crate::app::apptrait::App;
use crate::app::events::{AftertouchEvent, MidiEvent, SurfaceEvent};
use crate::driver;
use crate::sys::led;
use crate::sys::midi::MidiPort;
#[cfg(feature = "launchpad-pro-mk3")]
use crate::sys::rotation;
use crate::sys::settings;
use crate::utils::layout;

pub struct PerformanceApp;

impl PerformanceApp {
    pub const fn new() -> Self {
        Self
    }
}

impl App for PerformanceApp {
    fn on_enter(&mut self) {}

    fn on_exit(&mut self) {}

    fn on_surface(&mut self, event: SurfaceEvent) {
        #[cfg(feature = "launchpad-pro-mk3")]
        let index =
            if settings::with(|s| s.mirror_enabled) != 0 && (100..=110).contains(&event.index) {
                rotation::to_canonical(event.index - 100)
            } else {
                event.index
            };
        #[cfg(not(feature = "launchpad-pro-mk3"))]
        let index = event.index;

        let note = layout::xy_to_dr(index);
        if note == 0 {
            return;
        }

        let velocity = settings::with(|settings| {
            if event.pressed {
                if settings.velocity_enabled != 0 {
                    apply_velocity_curve(settings.velocity_curve, event.value)
                } else {
                    127
                }
            } else {
                0
            }
        });

        driver::send_midi(MidiPort::Midi, &[0x90, note, velocity]);
    }

    fn on_midi(&mut self, event: MidiEvent) {
        if event.port != MidiPort::Midi {
            return;
        }

        let led_index = layout::dr_to_xy(event.data1);

        let status_type = event.status & 0xf0;
        let _channel = event.status & 0x0f;

        #[cfg(any(
            feature = "launchpad-x",
            feature = "launchpad-mini-mk3",
            feature = "launchpad-pro-mk3"
        ))]
        let channel_match = _channel == 0;

        #[cfg(not(any(
            feature = "launchpad-x",
            feature = "launchpad-mini-mk3",
            feature = "launchpad-pro-mk3"
        )))]
        let channel_match = true;

        match status_type {
            0x90 => {
                if channel_match && led_index != 0 {
                    led::set_palette(led_index, event.data2);
                    #[cfg(feature = "launchpad-pro-mk3")]
                    if settings::with(|s| s.mirror_enabled) != 0 {
                        let raw_led = rotation::to_raw(led_index);
                        if (1..=8).contains(&raw_led) {
                            led::set_palette_raw(raw_led + 100, event.data2);
                        }
                    }
                }
            }
            0x80 => {
                if channel_match && led_index != 0 {
                    led::set_palette(led_index, 0);
                    #[cfg(feature = "launchpad-pro-mk3")]
                    if settings::with(|s| s.mirror_enabled) != 0 {
                        let raw_led = rotation::to_raw(led_index);
                        if (1..=8).contains(&raw_led) {
                            led::set_palette_raw(raw_led + 100, 0);
                        }
                    }
                }
            }
            _ => {
                if event.status == 0xb0 {
                    #[cfg(any(
                        feature = "launchpad-x",
                        feature = "launchpad-mini-mk3",
                        feature = "launchpad-pro-mk3"
                    ))]
                    if (90..=99).contains(&event.data1) {
                        led::set_palette(event.data1, event.data2);
                    }

                    #[cfg(not(any(
                        feature = "launchpad-x",
                        feature = "launchpad-mini-mk3",
                        feature = "launchpad-pro-mk3"
                    )))]
                    led::set_palette(event.data1, event.data2);
                }
            }
        }
    }

    fn on_aftertouch(&mut self, event: AftertouchEvent) {
        #[cfg(feature = "launchpad-pro-mk3")]
        let index =
            if settings::with(|s| s.mirror_enabled) != 0 && (100..=110).contains(&event.index) {
                rotation::to_canonical(event.index - 100)
            } else {
                event.index
            };
        #[cfg(not(feature = "launchpad-pro-mk3"))]
        let index = event.index;

        let note = layout::xy_to_dr(index);
        if note == 0 {
            return;
        }

        match settings::with(|settings| settings.aftertouch_mode) {
            1 => driver::send_midi(MidiPort::Midi, &[0xa0, note, event.value]),
            2 => driver::send_midi(MidiPort::Midi, &[0xd0, event.value]),
            _ => {}
        }
    }

    fn on_tick(&mut self) {}
}

pub(crate) fn apply_velocity_curve(curve: u8, value: u8) -> u8 {
    match curve {
        0 => velocity_curve_low(value),
        2 => velocity_curve_high(value),
        _ => value.min(127),
    }
}

fn velocity_curve_low(value: u8) -> u8 {
    if value == 0 {
        return 0;
    }

    let x = value.min(127) as u16;
    let y = (x * x + 63) / 127;
    y.min(127) as u8
}

fn velocity_curve_high(value: u8) -> u8 {
    if value == 0 {
        return 0;
    }

    let mut n = (value.min(127) as u16) * 127;
    let mut res = 0u16;
    let mut bit = 1u16 << 14;

    while bit > n {
        bit >>= 2;
    }

    while bit != 0 {
        if n >= res + bit {
            n -= res + bit;
            res = (res >> 1) + bit;
        } else {
            res >>= 1;
        }
        bit >>= 2;
    }

    res.min(127) as u8
}
