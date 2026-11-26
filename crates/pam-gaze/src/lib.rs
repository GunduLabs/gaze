// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(clippy::missing_safety_doc)]
pub mod auth;
pub use auth::*;
pub mod core;
pub use core::*;

use std::ffi::CStr;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::raw::{c_char, c_int};

const GROSSHACK_MODULE: &[u8] = b"pam_gaze_grosshack.so";

pub const DEPRECATION_NOTICE: &str = "\x1b[1;33m[Gaze Notice]\x1b[0m pam_gaze_grosshack.so is deprecated and will be removed in a future release.\n\
    Please update your PAM configuration to use: pam_gaze.so simultaneous\n\
    Run 'gaze doctor' to check your configuration.\n";

fn emit_deprecation_notice() {
    if let Ok(mut tty) = OpenOptions::new().write(true).open("/dev/tty") {
        let _ = tty.write_all(DEPRECATION_NOTICE.as_bytes());
    } else {
        eprint!("{DEPRECATION_NOTICE}");
    }
}

fn is_grosshack_path(path: &[u8]) -> bool {
    path.rsplit(|byte| *byte == b'/').next() == Some(GROSSHACK_MODULE)
}

fn loaded_as_grosshack() -> bool {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    let symbol = pam_sm_authenticate as *const libc::c_void;
    if unsafe { libc::dladdr(symbol, &mut info) } == 0 || info.dli_fname.is_null() {
        return false;
    }
    is_grosshack_path(unsafe { CStr::from_ptr(info.dli_fname) }.to_bytes())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: PamHandle,
    flags: c_int,
    argc: c_int,
    argv: *const *const c_char,
) -> c_int {
    guard_pam_entry(|| {
        let mut options = unsafe { parse_raw_pam_options(argc, argv) };
        if loaded_as_grosshack() {
            emit_deprecation_notice();
            options.mode = PamMode::Simultaneous;
        }
        unsafe { do_authenticate(pamh, flags, options) }
    })
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

    #[test]
    fn grosshack_path_matches_only_the_module_file_name() {
        assert!(is_grosshack_path(
            b"/usr/lib/security/pam_gaze_grosshack.so"
        ));
        assert!(is_grosshack_path(b"pam_gaze_grosshack.so"));
        assert!(!is_grosshack_path(b"/usr/lib/security/pam_gaze.so"));
        assert!(!is_grosshack_path(
            b"/opt/pam_gaze_grosshack.so.d/pam_gaze.so"
        ));
    }

    #[test]
    fn test_binary_is_not_loaded_as_grosshack() {
        assert!(!loaded_as_grosshack());
    }
}
