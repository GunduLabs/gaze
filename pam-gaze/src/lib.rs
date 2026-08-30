// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(clippy::missing_safety_doc)]
pub mod auth;
pub use auth::*;
pub mod core;
pub use core::*;

use std::os::raw::{c_char, c_int};

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: PamHandle,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    let mode = unsafe { parse_raw_pam_mode(argc, argv) };
    unsafe { do_authenticate(pamh, flags, mode) }
}

pam_success_stubs!();
