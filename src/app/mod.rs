// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

pub mod apptrait;
pub mod boot;
pub mod daw;
pub mod events;
pub mod host;
pub mod palette_editor;
pub mod performance;
pub mod programmer;
pub mod setup;

pub use crate::sys::midi::MidiPort;
pub use apptrait::App;
pub use boot::{BootAnimationApp, BootApp};
pub use events::{AftertouchEvent, MidiEvent, SurfaceEvent};
pub use host::AppHost;

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum AppId {
    Boot,
    Setup,
    Performance,
    Programmer,
    Daw,
    PaletteEditor,
}
