// SPDX-License-Identifier: GPL-3.0-only
// Copyright (C) 2025-2026 Anthony Hofmeister

#[cfg(any(
    feature = "launchpad-s",
    feature = "launchpad-mini-mk1",
    feature = "launchpad-mk2",
    feature = "launchpad-pro"
))]
pub mod pma;

#[cfg(any(
    feature = "launchpad-s",
    feature = "launchpad-mini-mk1",
    feature = "launchpad-mk2",
    feature = "launchpad-pro"
))]
pub use pma::{init, init_with_mode, poll, pump_tx, reinit_mode};

#[cfg(any(
    feature = "launchpad-mini-mk3",
    feature = "launchpad-x",
    feature = "launchpad-pro-mk3"
))]
pub mod otg_fs;

#[cfg(any(
    feature = "launchpad-mini-mk3",
    feature = "launchpad-x",
    feature = "launchpad-pro-mk3"
))]
pub use otg_fs::{init, init_with_mode, poll, pump_tx, reinit_mode};
