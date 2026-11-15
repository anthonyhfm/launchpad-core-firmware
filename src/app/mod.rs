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
#[cfg(feature = "remap")]
pub mod remap;
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
    /// User-defined button remapping (only built with the `remap` feature).
    Remap,
}

/// Mode to enter after the boot animation
/// If stored remap table exists -> Remap
/// Otherwise -> Performance
pub fn default_mode() -> AppId {
    #[cfg(feature = "remap")]
    if crate::sys::remap::has_stored_table() {
        return AppId::Remap;
    }
    AppId::Performance
}
