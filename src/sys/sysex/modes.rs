// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

use crate::app::daw::protocol::{self, Command};
use crate::utils::device::Model;

// USB MIDI shares mode selection, but not DAW configuration or LED traffic.
pub(crate) fn is_selection(model: Model, data: &[u8]) -> bool {
    matches!(
        protocol::decode(model, data),
        Some(Command::Performance | Command::Layout(..) | Command::Live(_))
    )
}
