// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use crate::app::boot::BootApp;
use crate::app::daw::DawApp;
use crate::app::palette_editor::PaletteEditorApp;
use crate::app::performance::PerformanceApp;
use crate::app::programmer::ProgrammerApp;
use crate::app::setup::SetupApp;
use crate::app::{AftertouchEvent, App, AppId, MidiEvent, SurfaceEvent};
use crate::sys::led;
use crate::sys::midi::MidiPort;
use crate::sys::rotation;
#[cfg(not(feature = "no-setup-btn"))]
use crate::sys::settings;
use crate::sys::sysex::{self, modes};

const SETUP_HOLD_TICKS: u16 = 500;
#[cfg(feature = "no-setup-btn")]
const SETUP_HOLD_BUTTON_INDEX: u8 = 95;
#[cfg(not(feature = "no-setup-btn"))]
const SETUP_BUTTON_INDEX: u8 = 0;

pub struct AppHost {
    pub current: AppId,
    previous_app: AppId,
    boot: BootApp,
    setup: SetupApp,
    performance: PerformanceApp,
    programmer: ProgrammerApp,
    daw: DawApp,
    pending_host_switch: Option<AppId>,
    palette_editor: PaletteEditorApp,
    #[cfg(feature = "no-setup-btn")]
    setup_hold_ticks: u16,
    #[cfg(feature = "no-setup-btn")]
    setup_hold_active: bool,
    #[cfg(not(feature = "no-setup-btn"))]
    setup_button_ticks: u16,
    #[cfg(not(feature = "no-setup-btn"))]
    setup_button_active: bool,
}

impl AppHost {
    pub const fn new(current: AppId) -> Self {
        Self {
            current,
            boot: BootApp::new(),
            setup: SetupApp::new(),
            performance: PerformanceApp::new(),
            programmer: ProgrammerApp::new(),
            daw: DawApp::new(),
            pending_host_switch: None,
            palette_editor: PaletteEditorApp::new(),
            previous_app: current,
            #[cfg(feature = "no-setup-btn")]
            setup_hold_ticks: 0,
            #[cfg(feature = "no-setup-btn")]
            setup_hold_active: false,
            #[cfg(not(feature = "no-setup-btn"))]
            setup_button_ticks: 0,
            #[cfg(not(feature = "no-setup-btn"))]
            setup_button_active: false,
        }
    }

    fn active_app_mut(&mut self) -> &mut dyn App {
        match self.current {
            AppId::Boot => &mut self.boot,
            AppId::Setup => &mut self.setup,
            AppId::Performance => &mut self.performance,
            AppId::Programmer => &mut self.programmer,
            AppId::Daw => &mut self.daw,
            AppId::PaletteEditor => &mut self.palette_editor,
        }
    }

    pub fn init(&mut self) {
        self.active_app_mut().on_enter();
        self.apply_requested_app_switch();
    }

    #[inline(never)]
    pub fn switch(&mut self, app: AppId) {
        if app == self.current {
            return;
        }

        if app == AppId::Setup && self.current != AppId::PaletteEditor {
            self.previous_app = self.current;
        }

        self.active_app_mut().on_exit();

        self.current = app;

        led::clear();

        if app == AppId::Setup {
            self.setup.set_current_mode(self.previous_app);
        }

        self.active_app_mut().on_enter();
    }

    #[inline(never)]
    pub fn route_surface_event(&mut self, event: SurfaceEvent) {
        if event.index != 0 {
            let mut canonical_event = event;
            canonical_event.index = self.to_canonical_index(event.index);

            self.active_app_mut().on_surface(canonical_event);
            self.active_app_mut().on_surface_raw(event);
        }

        #[cfg(feature = "no-setup-btn")]
        if self.handle_setup_hold_button(&event) {
            return;
        }

        #[cfg(not(feature = "no-setup-btn"))]
        if self.handle_setup_button(&event) {
            return;
        }

        self.apply_requested_app_switch();
    }

    // DAW follows physical labels; Setup rotates only its main grid.
    fn to_canonical_index(&self, raw_index: u8) -> u8 {
        if self.current == AppId::Daw {
            raw_index
        } else if self.current == AppId::Setup {
            rotation::to_canonical_grid_only(raw_index)
        } else {
            rotation::to_canonical(raw_index)
        }
    }

    #[inline(never)]
    pub fn route_midi_event(&mut self, event: MidiEvent) {
        if event.port == MidiPort::Daw {
            if self.current == AppId::Daw && Self::handle_led_tempo_event(&event) {
                return;
            }
            self.daw.receive_midi(event);
            return;
        }

        if self.current == AppId::Daw {
            return;
        }

        if Self::handle_led_tempo_event(&event) {
            return;
        }

        self.active_app_mut().on_midi(event);
        self.apply_requested_app_switch();
    }

    #[inline(never)]
    pub fn route_aftertouch_event(&mut self, event: AftertouchEvent) {
        let mut canonical_event = event;
        canonical_event.index = self.to_canonical_index(event.index);

        self.active_app_mut().on_aftertouch(canonical_event);
        self.apply_requested_app_switch();
    }

    #[inline(never)]
    pub fn receive_sysex(&mut self, port: MidiPort, data: &[u8]) {
        if port == MidiPort::Daw {
            if sysex::device_inquiry::execute(self.current, port, data)
                || sysex::version_inquiry::execute(self.current, port, data)
            {
                return;
            }
            let (_, requested) = self.daw.receive_sysex_for_app(data, self.current);
            if let Some(app) = requested {
                self.request_host_switch(app);
            }
            return;
        }

        if port == MidiPort::Midi && modes::is_selection(self.daw.model(), data) {
            let (_, requested) = self.daw.receive_sysex_for_app(data, self.current);
            if let Some(app) = requested {
                self.request_host_switch(app);
            }
            return;
        }

        sysex::execute(self.current, port, data);
    }

    // Defer host requests until Boot or Setup finishes; the latest request wins.
    fn request_host_switch(&mut self, app: AppId) {
        if matches!(
            self.current,
            AppId::Boot | AppId::Setup | AppId::PaletteEditor
        ) {
            self.pending_host_switch = Some(app);
        } else {
            self.switch(app);
        }
    }

    #[inline(never)]
    pub fn route_tick_event(&mut self) {
        #[cfg(feature = "no-setup-btn")]
        self.tick_setup_hold_button();

        #[cfg(not(feature = "no-setup-btn"))]
        self.tick_setup_button();

        led::tick();
        self.active_app_mut().on_tick();
        self.apply_requested_app_switch();
    }

    fn apply_requested_app_switch(&mut self) {
        if let Some(app) = self.active_app_mut().take_requested_app_switch() {
            let app = if self.current == AppId::Boot {
                self.pending_host_switch.take().unwrap_or(app)
            } else {
                app
            };
            self.switch(app);
        }
    }

    fn handle_led_tempo_event(event: &MidiEvent) -> bool {
        match event.status {
            0xfa => {
                led::tempo_start();
                true
            }
            0xf8 => {
                led::tempo_midi_clock();
                true
            }
            0xfc => {
                led::tempo_stop();
                true
            }
            _ => false,
        }
    }

    fn exit_setup(&mut self) {
        let selected = self.setup.finish_setup();
        let pending = self.pending_host_switch.take();
        let app = selected.or(pending).unwrap_or(self.previous_app);
        self.switch(app);
    }

    #[cfg(feature = "no-setup-btn")]
    fn handle_setup_hold_button(&mut self, event: &SurfaceEvent) -> bool {
        if event.index != SETUP_HOLD_BUTTON_INDEX {
            return false;
        }

        if self.current == AppId::PaletteEditor {
            return false;
        }

        if self.current == AppId::Boot {
            return true;
        }

        if self.current == AppId::Setup {
            if event.pressed {
                self.exit_setup();
            }

            return true;
        }

        if event.pressed {
            self.setup_hold_ticks = 0;
            self.setup_hold_active = true;
        } else {
            self.setup_hold_active = false;
            self.setup_hold_ticks = 0;
        }

        true
    }

    #[cfg(not(feature = "no-setup-btn"))]
    fn handle_setup_button(&mut self, event: &SurfaceEvent) -> bool {
        if event.index != SETUP_BUTTON_INDEX {
            return false;
        }

        if self.current == AppId::Boot {
            return true;
        }

        if event.pressed {
            if self.current == AppId::Setup {
                self.setup_button_active = false;
                self.setup_button_ticks = 0;
                self.exit_setup();
            } else if self.current == AppId::PaletteEditor {
                self.setup_button_active = false;
                self.setup_button_ticks = 0;
                led::clear();
                settings::save();
                self.switch(AppId::Setup);
            } else {
                self.setup_button_ticks = 0;
                self.setup_button_active = true;
                self.switch(AppId::Setup);
            }
        } else {
            let was_active = self.setup_button_active;
            self.setup_button_active = false;

            if was_active && self.setup_button_ticks >= SETUP_HOLD_TICKS {
                self.exit_setup();
            }
            self.setup_button_ticks = 0;
        }

        true
    }

    #[cfg(not(feature = "no-setup-btn"))]
    fn tick_setup_button(&mut self) {
        if self.setup_button_active {
            self.setup_button_ticks = self.setup_button_ticks.saturating_add(1);
        }
    }

    #[cfg(feature = "no-setup-btn")]
    fn tick_setup_hold_button(&mut self) {
        if !self.setup_hold_active {
            return;
        }

        self.setup_hold_ticks = self.setup_hold_ticks.saturating_add(1);

        if self.setup_hold_ticks >= SETUP_HOLD_TICKS {
            self.setup_hold_active = false;
            self.setup_hold_ticks = 0;

            self.route_surface_event(SurfaceEvent {
                pressed: false,
                index: 95,
                value: 0,
            });

            self.switch(AppId::Setup);
        }
    }
}
