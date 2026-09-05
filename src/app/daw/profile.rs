// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister

use crate::utils::device::Model;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Session,
    Note,
    Chord,
    Custom,
    Sequencer,
    Projects,
    Drums,
    Keys,
    User,
}

pub const fn modern(model: Model) -> bool {
    matches!(model, Model::ProMk3 | Model::X | Model::MiniMk3)
}

pub const fn standalone_page(model: Model) -> Page {
    match model {
        Model::ProMk3 | Model::X => Page::Note,
        Model::MiniMk3 => Page::Keys,
        _ => Page::Session,
    }
}

pub const fn standalone_layout(model: Model) -> u8 {
    match model {
        Model::ProMk3 => 4,
        Model::X => 1,
        Model::MiniMk3 => 5,
        _ => 0,
    }
}

pub const fn banks(model: Model) -> usize {
    match model {
        Model::ProMk3 => 4,
        Model::Basic => 0,
        _ => 1,
    }
}

pub fn page_button(model: Model, index: u8) -> Option<Page> {
    use Page::*;
    match (model, index) {
        (Model::ProMk3, 93) | (Model::X | Model::MiniMk3, 95) => Some(Session),
        (Model::ProMk3, 94) | (Model::X, 96) => Some(Note),
        (Model::ProMk3, 95) => Some(Chord),
        (Model::ProMk3, 96) | (Model::X, 97) => Some(Custom),
        (Model::ProMk3, 97) => Some(Sequencer),
        (Model::ProMk3, 98) => Some(Projects),
        (Model::MiniMk3, 96) => Some(Drums),
        (Model::MiniMk3, 97) => Some(Keys),
        (Model::MiniMk3, 98) => Some(User),
        _ => None,
    }
}
