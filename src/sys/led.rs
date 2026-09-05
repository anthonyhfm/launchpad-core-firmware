// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use crate::driver;
use crate::sys::rotation;
#[cfg(feature = "rgb-color")]
use crate::sys::settings;
#[cfg(feature = "rgb-color")]
use crate::utils::palette::{PALETTE_MAT1JACZYYY, PALETTE_MXOS, PALETTE_NOVATION};

const LED_COUNT: usize = if cfg!(feature = "launchpad-pro-mk3") {
    128
} else {
    100
};
const DEFAULT_TEMPO_BAR_TICKS: u32 = 2000;
const MIN_TEMPO_BAR_TICKS: u32 = 8;
const MIDI_CLOCKS_PER_BAR: u8 = 96;
const MIDI_CLOCKS_PER_BEAT: u8 = MIDI_CLOCKS_PER_BAR >> 2;

#[derive(Copy, Clone)]
struct LedColor {
    kind: u8,
    r: u8,
    g: u8,
    b: u8,
}

impl LedColor {
    const NONE: Self = Self {
        kind: 0,
        r: 0,
        g: 0,
        b: 0,
    };

    const fn rgb8(color: u32) -> Self {
        Self {
            kind: 1,
            r: ((color >> 16) & 0xff) as u8,
            g: ((color >> 8) & 0xff) as u8,
            b: (color & 0xff) as u8,
        }
    }

    const fn raw6(r: u8, g: u8, b: u8) -> Self {
        Self { kind: 2, r, g, b }
    }

    fn is_none(self) -> bool {
        self.kind == 0
    }

    fn is_black(self) -> bool {
        self.r == 0 && self.g == 0 && self.b == 0
    }

    fn scaled(self, factor: u8) -> Self {
        match self.kind {
            1 => Self {
                kind: 1,
                r: scale_channel(self.r, factor, 63),
                g: scale_channel(self.g, factor, 63),
                b: scale_channel(self.b, factor, 63),
            },
            2 => Self {
                kind: 2,
                r: scale_channel(self.r & 0x3f, factor, 63),
                g: scale_channel(self.g & 0x3f, factor, 63),
                b: scale_channel(self.b & 0x3f, factor, 63),
            },
            _ => Self::NONE,
        }
    }
}

struct LedState {
    base: [LedColor; LED_COUNT],
    flash: [LedColor; LED_COUNT],
    pulse: [LedColor; LED_COUNT],
    tempo_counter: u32,
    tempo_timer: u32,
    tempo_bar: u32,
    tempo_listen: bool,
    tempo_message_counter: u8,
}

impl LedState {
    const fn new() -> Self {
        Self {
            base: [LedColor::NONE; LED_COUNT],
            flash: [LedColor::NONE; LED_COUNT],
            pulse: [LedColor::NONE; LED_COUNT],
            tempo_counter: 0,
            tempo_timer: 0,
            tempo_bar: 0,
            tempo_listen: false,
            tempo_message_counter: 0,
        }
    }

    fn tempo_bar(&self) -> u32 {
        if self.tempo_bar == 0 {
            DEFAULT_TEMPO_BAR_TICKS
        } else {
            self.tempo_bar
        }
    }
}

struct LedStateSlot {
    state: core::cell::UnsafeCell<LedState>,
}

unsafe impl Sync for LedStateSlot {}

impl LedStateSlot {
    const fn new() -> Self {
        Self {
            state: core::cell::UnsafeCell::new(LedState::new()),
        }
    }

    fn with<R>(&self, f: impl FnOnce(&mut LedState) -> R) -> R {
        unsafe { f(&mut *self.state.get()) }
    }
}

static LED_STATE: LedStateSlot = LedStateSlot::new();

// Set now cares about orientation. This will not break the setup menu.
#[inline(never)]
pub fn set(index: u8, color: u32) {
    set_raw(rotation::to_raw(index), color);
}

// Same set but if you want to not mess with the orientation and light up a button regardless.
#[inline(never)]
pub fn set_raw(index: u8, color: u32) {
    update_base(index, LedColor::rgb8(color));
    driver::set_led(index, color);
}

// Same as the above two but for rgb
#[inline(never)]
pub fn set_rgb(index: u8, r: u8, g: u8, b: u8) {
    set_rgb_raw(rotation::to_raw(index), r, g, b);
}

#[inline(never)]
pub fn set_rgb_raw(index: u8, r: u8, g: u8, b: u8) {
    update_base(index, LedColor::raw6(r, g, b));
    driver::set_rgb_led(index, r, g, b);
}

pub fn set_palette(index: u8, velocity: u8) {
    set_palette_raw(rotation::to_raw(index), velocity);
}

#[inline(never)]
pub fn set_palette_raw(index: u8, velocity: u8) {
    #[cfg(feature = "rgb-color")]
    let (r, g, b) = settings::with(|settings| match settings.palette {
        0 => PALETTE_NOVATION.rgb(velocity),
        1 => PALETTE_MAT1JACZYYY.rgb(velocity),
        2 => PALETTE_MXOS.rgb(velocity),
        3 => rg_palette_rgb(velocity),
        4..=6 => {
            let slot = (settings.palette - 4) as usize;
            let velocity = (velocity as usize) & 0x7f;
            (
                settings.custom_palette[slot][0][velocity].min(63),
                settings.custom_palette[slot][1][velocity].min(63),
                settings.custom_palette[slot][2][velocity].min(63),
            )
        }
        _ => PALETTE_NOVATION.rgb(velocity),
    });

    #[cfg(not(feature = "rgb-color"))]
    let (r, g, b) = rg_palette_rgb(velocity);

    update_base(index, LedColor::raw6(r, g, b));
    driver::set_led_raw(index, r, g, b);
}

pub fn novation(index: u8, velocity: u8) {
    novation_raw(rotation::to_raw(index), velocity);
}

#[inline(never)]
pub fn novation_raw(index: u8, velocity: u8) {
    #[cfg(feature = "rgb-color")]
    let (r, g, b) = PALETTE_NOVATION.rgb(velocity);

    #[cfg(not(feature = "rgb-color"))]
    let (r, g, b) = rg_palette_rgb(velocity);

    update_base(index, LedColor::raw6(r, g, b));
    driver::set_led_raw(index, r, g, b);
}

/// Fixed DAW palette, independent of the Performance palette selection.
pub fn novation_rgb(value: u8) -> [u8; 3] {
    #[cfg(feature = "rgb-color")]
    let (r, g, b) = PALETTE_NOVATION.rgb(value);
    #[cfg(not(feature = "rgb-color"))]
    let (r, g, b) = rg_palette_rgb(value);
    [r, g, b]
}

/// Replace a DAW LED's complete state at its physical address.
pub fn daw_raw(index: u8, base: [u8; 3], flash: Option<[u8; 3]>, pulse: bool) {
    if !is_valid_index(index) {
        return;
    }
    LED_STATE.with(|state| {
        let i = index as usize;
        let base = LedColor::raw6(base[0], base[1], base[2]);
        state.base[i] = base;
        state.pulse[i] = if pulse { base } else { LedColor::NONE };
        state.flash[i] = flash
            .map(|c| LedColor::raw6(c[0], c[1], c[2]))
            .unwrap_or(LedColor::NONE);
        let bar = state.tempo_bar();
        let colour = if flash.is_some() && state.tempo_counter % (bar >> 2) < (bar >> 3) {
            state.flash[i]
        } else if pulse {
            base.scaled(pulse_factor(state.tempo_counter, bar))
        } else {
            base
        };
        render_color(index, colour);
    });
}

pub fn pulse(index: u8, color: u32) {
    pulse_raw(rotation::to_raw(index), color);
}

pub fn pulse_raw(index: u8, color: u32) {
    if !is_valid_index(index) {
        return;
    }

    let color = LedColor::rgb8(color);
    if color.is_black() {
        set_raw(index, 0);
        return;
    }

    update_pulse(index, color);
    render_pulse(index, color);
}

pub fn pulse_rgb(index: u8, r: u8, g: u8, b: u8) {
    pulse_rgb_raw(rotation::to_raw(index), r, g, b);
}

pub fn pulse_rgb_raw(index: u8, r: u8, g: u8, b: u8) {
    if !is_valid_index(index) {
        return;
    }

    let color = LedColor::raw6(r, g, b);
    if color.is_black() {
        set_rgb_raw(index, 0, 0, 0);
        return;
    }

    update_pulse(index, color);
    render_pulse(index, color);
}

pub fn flash(index: u8, color: u32) {
    flash_raw(rotation::to_raw(index), color);
}

pub fn flash_raw(index: u8, color: u32) {
    if !is_valid_index(index) {
        return;
    }

    let color = LedColor::rgb8(color);
    update_flash(index, color);
    render_flash(index, color);
}

pub fn flash_rgb(index: u8, r: u8, g: u8, b: u8) {
    flash_rgb_raw(rotation::to_raw(index), r, g, b);
}

pub fn flash_rgb_raw(index: u8, r: u8, g: u8, b: u8) {
    if !is_valid_index(index) {
        return;
    }

    let color = LedColor::raw6(r, g, b);
    update_flash(index, color);
    render_flash(index, color);
}

pub fn clear() {
    LED_STATE.with(|state| {
        state.base = [LedColor::NONE; LED_COUNT];
        state.flash = [LedColor::NONE; LED_COUNT];
        state.pulse = [LedColor::NONE; LED_COUNT];
    });
    driver::fill(0x0);
}

pub fn tick() {
    LED_STATE.with(|state| {
        state.tempo_timer = state.tempo_timer.saturating_add(1);

        let tempo_bar = state.tempo_bar();
        state.tempo_counter += 1;
        if state.tempo_counter >= tempo_bar {
            state.tempo_counter = 0;
        }

        let flash_period = tempo_bar >> 2;
        let flash_on = (state.tempo_counter % flash_period) < (tempo_bar >> 3);
        let pulse_factor = pulse_factor(state.tempo_counter, tempo_bar);

        for index in 0..LED_COUNT {
            let pulse = state.pulse[index];
            let flash = state.flash[index];
            if !flash.is_none() && !pulse.is_none() {
                render_color(
                    index as u8,
                    if flash_on { flash } else { pulse.scaled(pulse_factor) },
                );
                continue;
            }
            if !pulse.is_none() {
                render_color(index as u8, pulse.scaled(pulse_factor));
                continue;
            }

            let flash = state.flash[index];
            if !flash.is_none() {
                render_color(
                    index as u8,
                    if flash_on { flash } else { state.base[index] },
                );
            }
        }
    });
}

pub fn tempo_start() {
    LED_STATE.with(|state| {
        state.tempo_counter = 0;
        state.tempo_timer = 0;
        state.tempo_listen = true;
        state.tempo_message_counter = 0;
    });
}

pub fn tempo_midi_clock() {
    LED_STATE.with(|state| {
        if !state.tempo_listen {
            return;
        }

        state.tempo_message_counter = state.tempo_message_counter.saturating_add(1);

        if state.tempo_message_counter % MIDI_CLOCKS_PER_BEAT == 0 {
            if state.tempo_timer != 0 {
                state.tempo_bar = (state.tempo_timer << 2).max(MIN_TEMPO_BAR_TICKS);
            }
            state.tempo_timer = 0;
        }

        if state.tempo_message_counter >= MIDI_CLOCKS_PER_BAR {
            state.tempo_counter = 0;
            state.tempo_message_counter = 0;
        }
    });
}

pub fn tempo_stop() {
    LED_STATE.with(|state| {
        state.tempo_listen = false;
    });
}

fn update_base(index: u8, color: LedColor) {
    if !is_valid_index(index) {
        return;
    }

    LED_STATE.with(|state| {
        let index = index as usize;
        state.base[index] = color;
        state.flash[index] = LedColor::NONE;
        state.pulse[index] = LedColor::NONE;
    });
}

fn update_flash(index: u8, color: LedColor) {
    if !is_valid_index(index) {
        return;
    }

    LED_STATE.with(|state| {
        let index = index as usize;
        state.flash[index] = color;
        state.pulse[index] = LedColor::NONE;
    });
}

fn update_pulse(index: u8, color: LedColor) {
    if !is_valid_index(index) {
        return;
    }

    LED_STATE.with(|state| {
        let index = index as usize;
        state.pulse[index] = color;
        state.flash[index] = LedColor::NONE;
    });
}

fn render_flash(index: u8, color: LedColor) {
    LED_STATE.with(|state| {
        let tempo_bar = state.tempo_bar();
        let flash_period = tempo_bar >> 2;
        let flash_on = (state.tempo_counter % flash_period) < (tempo_bar >> 3);
        render_color(
            index,
            if flash_on {
                color
            } else {
                state.base[index as usize]
            },
        );
    });
}

fn render_pulse(index: u8, color: LedColor) {
    LED_STATE.with(|state| {
        let tempo_bar = state.tempo_bar();
        render_color(
            index,
            color.scaled(pulse_factor(state.tempo_counter, tempo_bar)),
        );
    });
}

fn is_valid_index(index: u8) -> bool {
    (index as usize) < LED_COUNT
}

fn render_color(index: u8, color: LedColor) {
    match color.kind {
        1 => {
            let color = ((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32);
            driver::set_led(index, color);
        }
        2 => driver::set_led_raw(index, color.r, color.g, color.b),
        _ => driver::set_led(index, 0),
    }
}

fn pulse_factor(tempo_counter: u32, tempo_bar: u32) -> u8 {
    let tempo_bar = if tempo_bar == 0 { DEFAULT_TEMPO_BAR_TICKS } else { tempo_bar };
    let half_bar = (tempo_bar >> 1).max(1);
    let t = tempo_counter % half_bar;

    if t < (tempo_bar >> 3) {
        ((15 * tempo_bar + 384 * t) / tempo_bar) as u8
    } else {
        ((237 * tempo_bar - 384 * t) / (3 * tempo_bar)) as u8
    }
}

fn scale_channel(value: u8, factor: u8, max: u8) -> u8 {
    (((value as u16) * (factor as u16)) / (max as u16)) as u8
}

fn rg_palette_rgb(velocity: u8) -> (u8, u8, u8) {
    (rg_calc(velocity, 0), rg_calc(velocity, 1), 0)
}

fn rg_calc(value: u8, index: u8) -> u8 {
    match index {
        0 => (value & 0x03) * 21,
        1 => ((value >> 4) & 0x03) * 21,
        _ => 0,
    }
}
