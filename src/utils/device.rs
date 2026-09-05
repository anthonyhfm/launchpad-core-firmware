// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2026 Anthony Hofmeister

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    ProMk3,
    X,
    MiniMk3,
    Mk2,
    Pro,
    Basic,
}

impl Model {
    pub const fn current() -> Self {
        if cfg!(feature = "launchpad-pro-mk3") {
            Self::ProMk3
        } else if cfg!(feature = "launchpad-x") {
            Self::X
        } else if cfg!(feature = "launchpad-mini-mk3") {
            Self::MiniMk3
        } else if cfg!(feature = "launchpad-mk2") {
            Self::Mk2
        } else if cfg!(feature = "launchpad-pro") {
            Self::Pro
        } else {
            Self::Basic
        }
    }

    pub const fn product(self) -> Option<u8> {
        match self {
            Self::ProMk3 => Some(0x0e),
            Self::X => Some(0x0c),
            Self::MiniMk3 => Some(0x0d),
            Self::Mk2 => Some(0x18),
            Self::Pro => Some(0x10),
            Self::Basic => None,
        }
    }

    pub fn valid_index(self, index: u8) -> bool {
        if is_grid(index) {
            return true;
        }
        match self {
            Self::ProMk3 => {
                (1..=8).contains(&index)
                    || (101..=108).contains(&index)
                    || (10..=90).contains(&index) && index % 10 == 0
                    || (19..=89).contains(&index) && index % 10 == 9
                    || (91..=99).contains(&index)
            }
            Self::Pro => {
                (1..=8).contains(&index)
                    || (10..=80).contains(&index) && index % 10 == 0
                    || (19..=89).contains(&index) && index % 10 == 9
                    || (91..=99).contains(&index)
            }
            Self::Mk2 | Self::Basic => {
                (91..=98).contains(&index) || (19..=89).contains(&index) && index % 10 == 9
            }
            _ => (91..=99).contains(&index) || (19..=89).contains(&index) && index % 10 == 9,
        }
    }

    pub fn is_cc(self, index: u8) -> bool {
        if self == Self::Mk2 {
            (91..=98).contains(&index)
        } else {
            !is_grid(index)
        }
    }

    pub fn wire_index(self, index: u8) -> u8 {
        if self == Self::Mk2 && (91..=98).contains(&index) {
            index + 13
        } else {
            index
        }
    }

    pub fn led_index(self, index: u8) -> u8 {
        if self == Self::ProMk3 && index == 99 {
            0
        } else {
            index
        }
    }

    pub fn physical_index(self, wire: u8, cc: bool) -> Option<u8> {
        let index = if self == Self::Mk2 && cc && (104..=111).contains(&wire) {
            wire - 13
        } else {
            wire
        };
        (self.valid_index(index) && self.is_cc(index) == cc).then_some(index)
    }
}

pub fn is_grid(index: u8) -> bool {
    (1..=8).contains(&(index / 10)) && (1..=8).contains(&(index % 10))
}
