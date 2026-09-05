// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

#[cfg(feature = "rgb-color")]
use crate::app::palette_editor::ensure_custom_palette_has_template;
use crate::app::setup::page::Page;
use crate::app::setup::text::Text;
use crate::app::{AppId, SurfaceEvent};
use crate::sys::led;
#[cfg(feature = "rgb-color")]
use crate::sys::settings;

const PERFORMANCE_BUTTON: u8 = 11;
const PROGRAMMER_BUTTON: u8 = 13;
const DAW_BUTTON: u8 = 12;
const PALETTE_RAINBOW_BUTTON: u8 = 25;
const CUSTOM_PALETTE_START: u8 = 26;
const SYSTEM_PALETTE_START: u8 = 15;

pub struct InitPage {
    text: Text,
    current_mode: AppId,
    selected_mode: Option<AppId>,
    requested_app_switch: Option<AppId>,
    #[cfg(feature = "rgb-color")]
    palette_rainbow_hue: u8,
    #[cfg(feature = "rgb-color")]
    palette_rainbow_tick: u8,
}

impl InitPage {
    pub const fn new() -> Self {
        Self {
            text: Text::new(
                [0b11111101, 0b10100101, 0b10110111, 0b11100101],
                0b11000111,
                0xff0000,
                0xffcccc,
            ),
            current_mode: AppId::Performance,
            selected_mode: None,
            requested_app_switch: None,
            #[cfg(feature = "rgb-color")]
            palette_rainbow_hue: 0,
            #[cfg(feature = "rgb-color")]
            palette_rainbow_tick: 0,
        }
    }

    pub fn set_current_mode(&mut self, app: AppId) {
        if matches!(app, AppId::Performance | AppId::Programmer | AppId::Daw) {
            self.current_mode = app;
            self.selected_mode = None;
        }
    }

    pub fn take_selected_mode(&mut self) -> Option<AppId> {
        self.selected_mode.take()
    }

    pub fn take_requested_app_switch(&mut self) -> Option<AppId> {
        self.requested_app_switch.take()
    }

    fn active_mode(&self) -> AppId {
        self.selected_mode.unwrap_or(self.current_mode)
    }

    fn draw_mode_selector(&self) {
        let mode = self.active_mode();
        led::set(
            DAW_BUTTON,
            if mode == AppId::Daw {
                0x00ff00
            } else {
                0x004000
            },
        );

        led::set(
            PERFORMANCE_BUTTON,
            if mode == AppId::Performance {
                0x4000ff
            } else {
                0x100040
            },
        );

        led::set(
            PROGRAMMER_BUTTON,
            if mode == AppId::Programmer {
                0xff4000
            } else {
                0x401000
            },
        );
    }

    fn draw_palette_selector(&self) {
        #[cfg(feature = "rgb-color")]
        {
            let palette = settings::get().palette;

            if palette < 4 {
                led::set(PALETTE_RAINBOW_BUTTON, 0x000000);
            } else {
                led::set(PALETTE_RAINBOW_BUTTON, wheel(self.palette_rainbow_hue));
            }

            for slot in 0..3 {
                led::set(
                    CUSTOM_PALETTE_START + slot,
                    if palette == 4 + slot {
                        0x70eeff
                    } else {
                        0x106080
                    },
                );
            }

            for system_palette in 0..4 {
                led::set(
                    SYSTEM_PALETTE_START + system_palette,
                    if palette == system_palette {
                        0x6060ff
                    } else {
                        0x101090
                    },
                );
            }

            led::set(18, if palette == 3 { 0xffcc80 } else { 0x906010 });
        }
        #[cfg(not(feature = "rgb-color"))]
        {
            led::set(PALETTE_RAINBOW_BUTTON, 0x000000);
            for slot in 0..3 {
                led::set(CUSTOM_PALETTE_START + slot, 0x000000);
            }
            for system_palette in 0..4 {
                led::set(SYSTEM_PALETTE_START + system_palette, 0x000000);
            }
        }
    }

    #[cfg(feature = "rgb-color")]
    fn set_palette(&mut self, palette: u8) {
        ensure_custom_palette_has_template(palette);

        settings::update(|settings| {
            settings.palette = palette;
        });
        self.draw_palette_selector();
    }
}

impl Page for InitPage {
    fn on_enter(&mut self) {
        led::set_raw(89, 0x550000);

        self.text.draw();
        led::set(57, 0x100000);
        self.draw_mode_selector();
        self.draw_palette_selector();
    }

    fn on_surface(&mut self, event: SurfaceEvent) {
        if !event.pressed {
            return;
        }

        match event.index {
            DAW_BUTTON => {
                self.selected_mode = Some(AppId::Daw);
                self.draw_mode_selector();
            }
            PERFORMANCE_BUTTON => {
                self.selected_mode = Some(AppId::Performance);
                self.draw_mode_selector();
            }
            PROGRAMMER_BUTTON => {
                self.selected_mode = Some(AppId::Programmer);
                self.draw_mode_selector();
            }
            #[cfg(feature = "rgb-color")]
            15..=18 => self.set_palette(event.index - SYSTEM_PALETTE_START),
            #[cfg(feature = "rgb-color")]
            26..=28 => self.set_palette(event.index - CUSTOM_PALETTE_START + 4),
            #[cfg(feature = "rgb-color")]
            PALETTE_RAINBOW_BUTTON if settings::get().palette >= 4 => {
                self.requested_app_switch = Some(AppId::PaletteEditor);
            }
            _ => {}
        }
    }

    fn on_tick(&mut self) {
        #[cfg(feature = "rgb-color")]
        {
            if settings::get().palette < 4 {
                return;
            }

            self.palette_rainbow_tick = self.palette_rainbow_tick.saturating_add(1);
            if self.palette_rainbow_tick < 6 {
                return;
            }

            self.palette_rainbow_tick = 0;
            self.palette_rainbow_hue = self.palette_rainbow_hue.wrapping_add(1);
            led::set(PALETTE_RAINBOW_BUTTON, wheel(self.palette_rainbow_hue));
        }
    }
}

#[cfg(feature = "rgb-color")]
fn wheel(pos: u8) -> u32 {
    if pos < 85 {
        let g = pos as u32 * 3;
        return ((255 - g) << 16) | (g << 8);
    }

    if pos < 170 {
        let pos = (pos - 85) as u32;
        let b = pos * 3;
        return ((255 - b) << 8) | b;
    }

    let pos = (pos - 170) as u32;
    let r = pos * 3;
    r << 16 | (255 - r)
}
