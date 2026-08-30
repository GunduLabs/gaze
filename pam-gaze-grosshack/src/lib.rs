// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(clippy::missing_safety_doc)]
#[path = "../../pam-gaze/src/core.rs"]
pub mod core;
pub use core::*;

#[path = "../../pam-gaze/src/auth.rs"]
pub mod auth;
pub use auth::*;

use std::fs::OpenOptions;
use std::io::Write;
use std::os::raw::{c_char, c_int};

pub const DEPRECATION_NOTICE: &str = "\x1b[1;33m[Gaze Notice]\x1b[0m pam_gaze_grosshack.so is deprecated and will be removed in a future release.\n\
    Please update your PAM configuration to use: pam_gaze.so simultaneous\n\
    Run 'gaze doctor' to check your configuration.\n";

pub fn emit_deprecation_notice() {
    if let Ok(mut tty) = OpenOptions::new().write(true).open("/dev/tty") {
        let _ = tty.write_all(DEPRECATION_NOTICE.as_bytes());
    } else {
        eprint!("{DEPRECATION_NOTICE}");
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: PamHandle,
    flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    emit_deprecation_notice();
    unsafe { do_authenticate(pamh, flags, PamMode::Simultaneous) }
}

pam_success_stubs!();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deprecation_notice_contains_expected_guidance() {
        assert!(DEPRECATION_NOTICE.contains("pam_gaze_grosshack.so is deprecated"));
        assert!(DEPRECATION_NOTICE.contains("pam_gaze.so simultaneous"));
        assert!(DEPRECATION_NOTICE.contains("gaze doctor"));
    }
}
