// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

pub mod device_inquiry;
pub mod fastled;
pub mod led_control;
pub mod modes;
pub mod palette;
#[cfg(feature = "launchpad-pro-mk3")]
pub mod roadrunner;
pub mod version_inquiry;

use crate::app::AppId;
use crate::sys::midi::MidiPort;

#[inline(never)]
pub fn execute(app: AppId, port: MidiPort, data: &[u8]) -> bool {
    #[cfg(feature = "launchpad-pro-mk3")]
    if roadrunner::execute(app, port, data) {
        return true;
    }

    #[cfg(feature = "remap")]
    if crate::sys::remap::execute(port, data) {
        return true;
    }

    if device_inquiry::execute(app, port, data) {
        return true;
    }

    if app == AppId::Performance && (fastled::execute(app, port, data) || led_control::execute(app, port, data)) {
        return true;
    }

    if version_inquiry::execute(app, port, data) {
        return true;
    }

    if palette::execute(app, port, data) {
        return true;
    }

    false
}
