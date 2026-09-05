// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister

mod fader;
pub mod profile;
pub mod protocol;

use crate::app::{AftertouchEvent, App, AppId, MidiEvent, SurfaceEvent};
use crate::driver;
use crate::sys::midi::MidiPort;
use crate::sys::{led, settings};
use crate::utils::device::{Model, is_grid};
use fader::Bank;
use profile::Page;
use protocol::Command;

const BANKS: usize = profile::banks(Model::current());

#[derive(Clone, Copy)]
struct Light {
    base: [u8; 3],
    flash: [u8; 3],
    flashing: bool,
    pulsing: bool,
}

impl Light {
    const OFF: Self = Self {
        base: [0; 3],
        flash: [0; 3],
        flashing: false,
        pulsing: false,
    };

    fn palette(&mut self, channel: u8, value: u8) {
        let rgb = led::novation_rgb(value);
        match channel {
            0 => {
                *self = Self {
                    base: rgb,
                    ..Self::OFF
                };
            }
            1 => {
                self.flash = rgb;
                self.flashing = true;
            }
            2 => {
                self.base = rgb;
                self.pulsing = true;
                self.flashing = false;
            }
            _ => {}
        }
    }
}

pub struct DawApp {
    enabled: bool,
    visible: bool,
    model: Model,
    page: Page,
    layout: u8,
    subpage: u8,
    bank: Option<u8>,
    banks: [Bank; BANKS],
    session_framebuffer: [Light; 128],
    cc_values: [u8; 128],
    held: [bool; 128],
    session_colours: [u8; 2],
}

impl DawApp {
    pub(crate) fn model(&self) -> Model {
        self.model
    }

    pub const fn new() -> Self {
        Self {
            enabled: false,
            visible: false,
            model: Model::current(),
            page: profile::standalone_page(Model::current()),
            layout: profile::standalone_layout(Model::current()),
            subpage: 0,
            bank: None,
            banks: [Bank::new(); BANKS],
            session_framebuffer: [Light::OFF; 128],
            cc_values: [0; 128],
            held: [false; 128],
            session_colours: [0; 2],
        }
    }

    fn reset(&mut self) {
        self.release_all();
        self.stop();
        self.enabled = false;
        self.page = profile::standalone_page(self.model);
        self.layout = profile::standalone_layout(self.model);
        self.subpage = 0;
        self.bank = None;
        self.session_framebuffer = [Light::OFF; 128];
        self.banks = [Bank::new(); BANKS];
        self.cc_values = [0; 128];
        self.session_colours = [0; 2];
    }

    fn stop(&mut self) {
        for b in &mut self.banks {
            b.stop();
        }
    }

    fn send(&self, index: u8, value: u8) {
        driver::send_midi(
            MidiPort::Daw,
            &[
                if self.model.is_cc(index) { 0xb0 } else { 0x90 },
                self.model.wire_index(index),
                value,
            ],
        );
    }

    fn release_all(&mut self) {
        for index in 0..128 {
            if self.held[index] {
                self.send(index as u8, 0);
                self.held[index] = false;
            }
        }
    }

    fn change_view(&mut self, page: Page, layout: u8, subpage: u8, bank: Option<u8>) {
        if (self.page, self.layout, self.subpage, self.bank) == (page, layout, subpage, bank) {
            return;
        }

        for index in 0..128 {
            if self.held[index]
                && profile::page_button(self.model, index as u8).is_none()
                && !(self.model == Model::X && index == 98)
            {
                self.send(index as u8, 0);
                self.held[index] = false;
            }
        }

        self.stop();
        self.page = page;
        self.layout = layout;
        self.subpage = subpage;
        self.bank = bank;

        if self.model == Model::ProMk3 {
            self.reply(0, &[layout, subpage, 0]);
        }

        self.render();
    }

    fn select_page(&mut self, page: Page) {
        let layout = match (self.model, page) {
            (_, Page::Session) => 0,
            (Model::ProMk3, Page::Note) => 4,
            (Model::ProMk3, Page::Chord) => 2,
            (Model::ProMk3, Page::Custom) => 3,
            (Model::ProMk3, Page::Sequencer) => 7,
            (Model::ProMk3, Page::Projects) => 0x0d,
            (_, Page::Note) => 1,
            (_, Page::Custom | Page::Drums) => 4,
            (_, Page::Keys) => 5,
            (_, Page::User) => 6,
            _ => 0,
        };
        self.change_view(page, layout, 0, None);
    }

    fn reply(&self, command: u8, payload: &[u8]) {
        let Some(product) = self.model.product() else {
            return;
        };
        let mut response = [0u8; 48];
        response[..7].copy_from_slice(&[0xf0, 0, 0x20, 0x29, 2, product, command]);
        response[7..7 + payload.len()].copy_from_slice(payload);
        response[7 + payload.len()] = 0xf7;
        driver::send_midi(MidiPort::Daw, &response[..8 + payload.len()]);
    }

    fn query(&self, command: u8) {
        match command {
            0 if self.model == Model::ProMk3 => self.reply(0, &[self.layout, self.subpage, 0]),
            0 => self.reply(0, &[self.layout]),
            0x10 => self.reply(command, &[self.enabled as u8]),
            0x0e => self.reply(command, &[0]),
            0x14 => self.reply(command, &self.session_colours),
            1 if BANKS > 0 => {
                let b = &self.banks[0];
                let mut p = [0; 34];
                p[1] = b.horizontal as u8;
                for (i, f) in b.faders.iter().enumerate() {
                    p[2 + i * 4..6 + i * 4].copy_from_slice(&[
                        i as u8,
                        f.bipolar as u8,
                        f.cc,
                        f.colour,
                    ]);
                }
                self.reply(1, &p);
            }
            _ => {}
        }
    }

    pub fn receive_sysex_for_app(&mut self, data: &[u8], current: AppId) -> (bool, Option<AppId>) {
        if current == AppId::Programmer {
            match protocol::decode(self.model, data) {
                Some(Command::Query(0x0e)) => {
                    self.reply(0x0e, &[1]);
                    return (true, None);
                }
                Some(Command::Query(0)) => {
                    if self.model == Model::ProMk3 {
                        self.reply(0, &[0x11, 0, 0]);
                    } else {
                        self.reply(0, &[0x7f]);
                    }
                    return (true, None);
                }
                _ => {}
            }
        }

        self.receive_sysex(data)
    }

    fn receive_sysex(&mut self, data: &[u8]) -> (bool, Option<AppId>) {
        let Some(command) = protocol::decode(self.model, data) else {
            return (false, None);
        };

        let mut switch = None;

        match command {
            Command::Enable(true) => {
                self.enabled = true;
                switch = Some(AppId::Daw);
            }
            Command::Enable(false) => {
                self.reset();
                switch = Some(AppId::Daw);
            }
            Command::Query(cmd) => self.query(cmd),
            Command::Performance => {
                self.release_all();
                self.stop();
                switch = Some(AppId::Performance);
            }
            Command::Live(false) => {
                self.release_all();
                self.stop();
                switch = Some(AppId::Programmer);
            }
            Command::Live(true) => {
                if self.enabled {
                    self.select_page(Page::Session);
                    switch = Some(AppId::Daw);
                } else {
                    self.select_page(profile::standalone_page(self.model));
                    switch = Some(AppId::Daw);
                }
            }
            Command::Layout(layout, subpage) => {
                if (self.model == Model::ProMk3 && layout == 0x11)
                    || (profile::modern(self.model) && layout == 0x7f)
                    || (self.model == Model::Pro && layout == 3)
                {
                    self.release_all();
                    self.stop();
                    switch = Some(AppId::Programmer);
                } else if profile::modern(self.model)
                    && !self.enabled
                    && (layout == 0
                        || (self.model == Model::ProMk3 && layout == 1)
                        || (self.model != Model::ProMk3 && layout == 0x0d))
                {
                    return (true, None);
                } else {
                    if !profile::modern(self.model) {
                        self.enabled = true;
                    }
                    let (page, bank) = match (self.model, layout) {
                        (Model::ProMk3, 1) => (Page::Session, Some(subpage)),
                        (Model::X | Model::MiniMk3, 0x0d) => (Page::Session, Some(0)),
                        (Model::Mk2, 4 | 5) | (Model::Pro, 2) => (Page::Session, Some(0)),
                        (_, 0) => (Page::Session, None),
                        (Model::ProMk3, 2) => (Page::Chord, None),
                        (Model::ProMk3, 3) => (Page::Custom, None),
                        (Model::ProMk3, 4) | (Model::X, 1) => (Page::Note, None),
                        (Model::ProMk3, 7) => (Page::Sequencer, None),
                        (Model::ProMk3, 0x0d) => (Page::Projects, None),
                        (Model::MiniMk3, 4) => (Page::Drums, None),
                        (Model::MiniMk3, 5) => (Page::Keys, None),
                        (Model::MiniMk3, 6) => (Page::User, None),
                        _ => (Page::Custom, None),
                    };
                    if self.model == Model::Pro && layout == 2 {
                        if let Some(b) = self.banks.first_mut() {
                            for f in &mut b.faders {
                                f.colour = 0;
                            }
                        }
                    }
                    self.change_view(page, layout, subpage, bank);
                    switch = Some(AppId::Daw);
                }
            }
            Command::Faders(bank, horizontal, entries) => {
                let b = &mut self.banks[bank as usize];
                if b.horizontal != horizontal {
                    b.stop();
                }
                b.horizontal = horizontal;
                for e in entries.chunks_exact(4) {
                    let f = &mut b.faders[e[0] as usize];
                    f.stop();
                    f.bipolar = e[1] != 0;
                    f.cc = e[2];
                    f.colour = e[3];
                    f.value = self.cc_values[e[2] as usize];
                }
            }
            Command::LegacyFaders(entries) => {
                if self.bank.is_none()
                    || self.model == Model::Mk2
                        && entries
                            .chunks_exact(4)
                            .any(|e| (e[1] != 0) != (self.layout == 5))
                {
                    return (true, None);
                }
                for e in entries.chunks_exact(4) {
                    let Some(b) = self.banks.first_mut() else {
                        return (true, None);
                    };
                    let f = &mut b.faders[e[0] as usize];
                    f.stop();
                    f.bipolar = e[1] != 0;
                    f.cc = 21 + e[0];
                    f.colour = e[2];
                    f.value = e[3];
                }
            }
            Command::Stop(bank) => self.banks[bank as usize].stop(),
            Command::SessionColours(a, b) => self.session_colours = [a, b],
            Command::Clear(notes, cc) => {
                for i in 0..128 {
                    if if self.model.is_cc(i as u8) { cc } else { notes } {
                        self.session_framebuffer[i] = Light::OFF;
                    }
                }
            }
            Command::Lights(mut p) => {
                while !p.is_empty() {
                    let len = match p[0] {
                        0 | 2 => 3,
                        1 => 4,
                        _ => 5,
                    };
                    if self.model.valid_index(p[1]) {
                        let light = &mut self.session_framebuffer[p[1] as usize];
                        match p[0] {
                            0 => light.palette(0, p[2]),
                            1 => {
                                light.palette(0, p[3]);
                                light.palette(1, p[2]);
                            }
                            2 => light.palette(2, p[2]),
                            _ => {
                                *light = Light {
                                    base: [p[2] >> 1, p[3] >> 1, p[4] >> 1],
                                    ..Light::OFF
                                }
                            }
                        }
                    }
                    p = &p[len..];
                }
            }
            Command::LegacyLights(cmd, p) => self.legacy_lights(cmd, p),
        }
        if !matches!(command, Command::Query(_)) {
            self.render();
        }
        (true, switch)
    }

    fn legacy_index(&self, index: u8) -> Option<u8> {
        let index = if self.model == Model::Mk2 && (104..=111).contains(&index) {
            index - 13
        } else {
            index
        };
        self.model.valid_index(index).then_some(index)
    }

    fn legacy_lights(&mut self, cmd: u8, p: &[u8]) {
        match cmd {
            0x0a | 0x0b | 0x23 | 0x28 => {
                for e in p.chunks_exact(if cmd == 0x0b { 4 } else { 2 }) {
                    if let Some(index) = self.legacy_index(e[0]) {
                        if cmd != 0x0b {
                            self.session_framebuffer[index as usize].palette(
                                match cmd {
                                    0x23 => 1,
                                    0x28 => 2,
                                    _ => 0,
                                },
                                e[1],
                            );
                        } else {
                            self.session_framebuffer[index as usize] = Light {
                                base: [e[1], e[2], e[3]],
                                ..Light::OFF
                            };
                        }
                    }
                }
            }
            0x0e => {
                for i in 0..99 {
                    self.session_framebuffer[i].palette(0, p[0]);
                }
            }
            0x0f => {
                let width = if p[0] == 1 {
                    8
                } else if self.model == Model::Mk2 {
                    9
                } else {
                    10
                };
                for (n, rgb) in p[1..].chunks_exact(3).enumerate() {
                    let (row, col) = ((n / width) as u8, (n % width) as u8);
                    let index = if p[0] == 1 {
                        (row + 1) * 10 + col + 1
                    } else if self.model == Model::Mk2 {
                        if row < 8 {
                            (row + 1) * 10 + col + 1
                        } else if col < 8 {
                            91 + col
                        } else {
                            continue;
                        }
                    } else {
                        row * 10 + col
                    };
                    if index != 99 && self.model.valid_index(index) {
                        self.session_framebuffer[index as usize] = Light {
                            base: [rgb[0], rgb[1], rgb[2]],
                            ..Light::OFF
                        };
                    }
                }
            }
            0x0c | 0x0d => {
                for (n, colour) in p[1..].iter().enumerate() {
                    let (row, col) = if cmd == 0x0c {
                        (n as u8, p[0])
                    } else {
                        (p[0], n as u8)
                    };
                    let index = if self.model == Model::Mk2 {
                        if row < 8 && col <= 8 {
                            (row + 1) * 10 + col + 1
                        } else if row == 8 && col < 8 {
                            91 + col
                        } else {
                            continue;
                        }
                    } else {
                        row * 10 + col
                    };
                    if self.model.valid_index(index) {
                        self.session_framebuffer[index as usize].palette(0, *colour);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn receive_midi(&mut self, event: MidiEvent) {
        if event.port != MidiPort::Daw || event.data1 > 127 || event.data2 > 127 {
            return;
        }
        let channel = event.status & 15;
        let kind = event.status & 0xf0;
        if kind == 0xb0
            && ((profile::modern(self.model) && matches!(channel, 4 | 5))
                || (!profile::modern(self.model)
                    && self.bank.is_some()
                    && channel == 0
                    && (21..=28).contains(&event.data1)))
        {
            if channel != 5 {
                self.cc_values[event.data1 as usize] = event.data2;
            }
            for b in &mut self.banks {
                for f in &mut b.faders {
                    if f.cc == event.data1 {
                        if channel == 5 {
                            f.colour = event.data2;
                            if f.colour == 0 {
                                f.stop();
                            }
                        } else if self.model != Model::Pro || !f.moving() {
                            f.stop();
                            f.value = event.data2;
                        }
                    }
                }
            }
            self.render_faders();
            return;
        }
        if channel > 2 || !matches!(kind, 0x80 | 0x90 | 0xb0) {
            return;
        }
        let Some(index) = self.model.physical_index(event.data1, kind == 0xb0) else {
            return;
        };
        self.session_framebuffer[index as usize]
            .palette(channel, if kind == 0x80 { 0 } else { event.data2 });
        self.render_index(index);
    }

    fn render_index(&self, index: u8) {
        if !self.visible || !self.model.valid_index(index) {
            return;
        }
        if let Some(page) = profile::page_button(self.model, index) {
            if page == Page::Session && !self.enabled {
                led::set_raw(index, 0);
            } else if page == Page::Session && self.session_colours[0] != 0 {
                led::novation_raw(
                    index,
                    self.session_colours[if self.page == page { 0 } else { 1 }],
                );
            } else {
                led::set_raw(
                    index,
                    if self.page == page {
                        0x80ff80
                    } else {
                        0x303030
                    },
                );
            }
        } else if self.page == Page::Session && !(self.bank.is_some() && is_grid(index))
            || self.model == Model::X && index == 98
        {
            let l = self.session_framebuffer[index as usize];
            led::daw_raw(
                self.model.led_index(index),
                l.base,
                l.flashing.then_some(l.flash),
                l.pulsing,
            );
        }
    }

    fn render_faders(&self) {
        if !self.visible || !self.enabled {
            return;
        }
        let Some(bank) = self.bank else {
            return;
        };
        let b = &self.banks[bank as usize];
        for (i, f) in b.faders.iter().enumerate() {
            Self::render_fader(b.horizontal, i, f);
        }
    }
    fn render_fader(horizontal: bool, i: usize, f: &fader::Fader) {
        let colour = led::novation_rgb(f.colour);
        for step in 0..8 {
            let index = if horizontal {
                (8 - i as u8) * 10 + step as u8 + 1
            } else {
                (step as u8 + 1) * 10 + i as u8 + 1
            };
            let brightness = f.brightness(step) as u16;
            let rgb = colour.map(|c| (c as u16 * brightness / 255) as u8);
            led::daw_raw(index, rgb, None, false);
        }
    }
    fn render(&self) {
        if !self.visible {
            return;
        }
        led::clear();
        for i in 0..128 {
            self.render_index(i);
        }
        self.render_faders();
    }
}

impl App for DawApp {
    fn on_enter(&mut self) {
        if !profile::modern(self.model) {
            self.enabled = true;
        }
        self.visible = true;
        self.render();
    }

    fn on_exit(&mut self) {
        self.release_all();
        self.stop();
        self.visible = false;
    }

    fn on_surface(&mut self, event: SurfaceEvent) {
        let index = event.index;
        if !self.model.valid_index(index) {
            return;
        }

        if !event.pressed {
            if self.held[index as usize] {
                self.send(index, 0);
                self.held[index as usize] = false;
            }
            return;
        }

        if self.held[index as usize] {
            return;
        }

        if let Some(page) = profile::page_button(self.model, index) {
            if page == Page::Session && !self.enabled {
                return;
            }
            self.send(index, 127);
            self.held[index as usize] = true;

            if self.page != page {
                self.select_page(page);
            }

            return;
        }

        if !self.enabled {
            return;
        }

        if self.page != Page::Session && !(self.model == Model::X && index == 98) {
            return;
        }

        if let Some(bank) = self.bank {
            if is_grid(index) {
                let b = &mut self.banks[bank as usize];
                let (fader, step) = if b.horizontal {
                    (8 - index / 10, index % 10 - 1)
                } else {
                    (index % 10 - 1, index / 10 - 1)
                };
                let f = &mut b.faders[fader as usize];

                if f.press(step as usize, event.value) {
                    driver::send_midi(
                        MidiPort::Daw,
                        &[
                            if profile::modern(self.model) {
                                0xb4
                            } else {
                                0xb0
                            },
                            f.cc,
                            f.value,
                        ],
                    );
                }
                return;
            }
        }

        let velocity = settings::with(|s| {
            if s.velocity_enabled != 0 {
                crate::app::performance::apply_velocity_curve(s.velocity_curve, event.value)
                    .clamp(1, 127)
            } else {
                127
            }
        });

        self.send(
            index,
            if self.model.is_cc(index) {
                127
            } else {
                velocity
            },
        );
        self.held[index as usize] = true;
    }

    fn on_midi(&mut self, event: MidiEvent) {
        self.receive_midi(event);
    }

    fn on_aftertouch(&mut self, event: AftertouchEvent) {
        if self.page != Page::Session
            || self.bank.is_some()
            || !is_grid(event.index)
            || !self.held[event.index as usize]
        {
            return;
        }
        match settings::with(|s| s.aftertouch_mode) {
            1 => driver::send_midi(MidiPort::Daw, &[0xa0, event.index, event.value.min(127)]),
            2 => driver::send_midi(MidiPort::Daw, &[0xd0, event.value.min(127)]),
            _ => {}
        }
    }

    fn on_tick(&mut self) {
        if !self.visible {
            return;
        }

        let Some(bank) = self.bank else {
            return;
        };

        let b = &mut self.banks[bank as usize];
        for (i, f) in b.faders.iter_mut().enumerate() {
            let moving = f.moving();
            if f.tick() {
                driver::send_midi(
                    MidiPort::Daw,
                    &[
                        if profile::modern(self.model) {
                            0xb4
                        } else {
                            0xb0
                        },
                        f.cc,
                        f.value,
                    ],
                );
                self.cc_values[f.cc as usize] = f.value;
            }

            if moving && f.animation_frame() {
                Self::render_fader(b.horizontal, i, f);
            }
        }
    }
}
