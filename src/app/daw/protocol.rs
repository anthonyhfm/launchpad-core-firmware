// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister

use super::profile;
use crate::utils::device::Model;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Performance,
    Enable(bool),
    Query(u8),
    Layout(u8, u8),
    Live(bool),
    Faders(u8, bool, &'a [u8]),
    LegacyFaders(&'a [u8]),
    Clear(bool, bool),
    SessionColours(u8, u8),
    Stop(u8),
    Lights(&'a [u8]),
    LegacyLights(u8, &'a [u8]),
}

pub fn decode(model: Model, data: &[u8]) -> Option<Command<'_>> {
    if data.len() < 8
        || data[..5] != [0xf0, 0, 0x20, 0x29, 2]
        || Some(data[5]) != model.product()
        || data.last() != Some(&0xf7)
        || data[6..data.len() - 1].iter().any(|b| *b > 127)
    {
        return None;
    }
    let cmd = data[6];
    let p = &data[7..data.len() - 1];
    if profile::modern(model) {
        match (cmd, p) {
            (0x00, [0x14, 0, 0]) if model == Model::ProMk3 => Some(Command::Performance),
            (0x00, [0x7e]) if matches!(model, Model::X | Model::MiniMk3) => {
                Some(Command::Performance)
            }
            (0x10, [v]) if *v <= 1 => Some(Command::Enable(*v != 0)),
            (0x00, []) => Some(Command::Query(cmd)),
            (0x01 | 0x10 | 0x14 | 0x0e, []) if model != Model::ProMk3 => Some(Command::Query(cmd)),
            (0x0e, [v]) if *v <= 1 => Some(Command::Live(*v == 0)),
            (0x00, [layout, page, 0])
                if model == Model::ProMk3 && valid_layout(model, *layout, *page) =>
            {
                Some(Command::Layout(*layout, *page))
            }
            (0x00, [layout]) if model != Model::ProMk3 && valid_layout(model, *layout, 0) => {
                Some(Command::Layout(*layout, 0))
            }
            (0x01, [bank, orientation, entries @ ..])
                if (*bank as usize) < profile::banks(model)
                    && *orientation <= 1
                    && faders_valid(entries) =>
            {
                Some(Command::Faders(*bank, *orientation != 0, entries))
            }
            (0x12, [session, drum, cc])
                if model != Model::ProMk3
                    && *session <= 1
                    && *drum <= 1
                    && *cc <= 1
                    && (model != Model::MiniMk3 || *drum == 0) =>
            {
                Some(Command::Clear(*session != 0, *cc != 0))
            }
            (0x14, [active, inactive]) if model != Model::ProMk3 => {
                Some(Command::SessionColours(*active, *inactive))
            }
            (0x19, [bank]) if model == Model::ProMk3 && *bank < 4 => Some(Command::Stop(*bank)),
            (0x03, entries)
                if lights_valid(entries, if model == Model::ProMk3 { 108 } else { 81 }) =>
            {
                Some(Command::Lights(entries))
            }
            _ => None,
        }
    } else {
        match (model, cmd, p) {
            (Model::Pro, 0x22, [3]) | (Model::Mk2, 0x22, [1]) => Some(Command::Performance),
            (Model::Pro, 0x22, [0]) => Some(Command::Layout(0, 0)),
            (Model::Mk2, 0x22, [layout @ (0 | 4 | 5)]) => Some(Command::Layout(*layout, 0)),
            (Model::Pro, 0x2c, [layout @ (2 | 3)]) => Some(Command::Layout(*layout, 0)),
            (Model::Mk2 | Model::Pro, 0x2b, entries) if faders_valid(entries) => {
                Some(Command::LegacyFaders(entries))
            }
            (_, 0x0a, entries)
                if !entries.is_empty() && entries.len() <= 194 && entries.len() % 2 == 0 =>
            {
                Some(Command::LegacyLights(cmd, entries))
            }
            (Model::Pro, 0x23 | 0x28, entries)
                if !entries.is_empty() && entries.len() <= 194 && entries.len() % 2 == 0 =>
            {
                Some(Command::LegacyLights(cmd, entries))
            }
            (_, 0x0b, entries)
                if !entries.is_empty()
                    && entries.len() % 4 == 0
                    && entries
                        .chunks_exact(4)
                        .all(|e| e[1..].iter().all(|v| *v <= 63)) =>
            {
                Some(Command::LegacyLights(cmd, entries))
            }
            (_, 0x0c | 0x0d, [axis, entries @ ..])
                if *axis <= 9 && !entries.is_empty() && entries.len() <= 10 =>
            {
                Some(Command::LegacyLights(cmd, p))
            }
            (_, 0x0e, [_]) => Some(Command::LegacyLights(cmd, p)),
            (_, 0x0f, [grid, entries @ ..])
                if *grid <= 1
                    && !entries.is_empty()
                    && entries.len() % 3 == 0
                    && entries.len()
                        <= if *grid == 1 {
                            192
                        } else if model == Model::Mk2 {
                            243
                        } else {
                            300
                        }
                    && entries.iter().all(|v| *v <= 63) =>
            {
                Some(Command::LegacyLights(cmd, p))
            }
            _ => None,
        }
    }
}

fn faders_valid(entries: &[u8]) -> bool {
    if entries.is_empty() || entries.len() > 32 || entries.len() % 4 != 0 {
        return false;
    }
    let mut seen = 0u8;
    for e in entries.chunks_exact(4) {
        if e[0] > 7 || e[1] > 1 || seen & (1 << e[0]) != 0 {
            return false;
        }
        seen |= 1 << e[0];
    }
    true
}

fn lights_valid(mut p: &[u8], max: usize) -> bool {
    let mut count = 0;
    while !p.is_empty() {
        let len = match p[0] {
            0 | 2 => 3,
            1 => 4,
            3 => 5,
            _ => return false,
        };
        if p.len() < len {
            return false;
        }
        p = &p[len..];
        count += 1;
    }
    count > 0 && count <= max
}

fn valid_layout(model: Model, layout: u8, page: u8) -> bool {
    match model {
        Model::ProMk3 => match layout {
            0 | 2 | 4 | 0x11 => page == 0,
            1 | 7 | 0x0d => page < 4,
            3 => page < 8,
            _ => false,
        },
        Model::X => matches!(layout, 0 | 1 | 4..=7 | 0x0d | 0x7f),
        Model::MiniMk3 => matches!(layout, 0 | 4..=6 | 0x0d | 0x7f),
        _ => false,
    }
}
