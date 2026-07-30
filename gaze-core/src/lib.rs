// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

pub mod camera;
pub mod config;
pub mod dbus;

#[cfg(feature = "detection")]
pub mod detect;
#[cfg(feature = "detection")]
pub mod face;
#[cfg(feature = "detection")]
pub mod inference;
pub mod ir;
