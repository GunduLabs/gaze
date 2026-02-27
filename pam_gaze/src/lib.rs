#![allow(clippy::missing_safety_doc)]
use gaze_core::capture::frame_to_bytes;
use pam_gaze_core::*;
use std::os::raw::{c_char, c_int};

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let username = match unsafe { get_username(pamh) } {
        Some(u) => u,
        None => return PAM_AUTH_ERR,
    };

    let (_, mut cam, proxy) = match setup_auth_env() {
        Ok(e) => e,
        Err(_) => {
            unsafe { say(pamh, "Face authentication unavailable") };
            return PAM_AUTHINFO_UNAVAIL;
        }
    };

    unsafe { say(pamh, "Please look at the camera") };

    for _ in 0..MAX_ATTEMPTS {
        let frame = match cam.capture_frame() {
            Ok(f) => f,
            Err(_) => continue,
        };
        let capture = match frame_to_bytes(&frame) {
            Ok(c) => c,
            Err(_) => continue,
        };

        match proxy.verify(&username, &capture.bytes, capture.width, capture.height) {
            Ok(true) => return PAM_SUCCESS,
            Ok(false) => {}
            Err(ref err) if is_retryable(err) => continue,
            Err(_) => return PAM_SERVICE_ERR,
        }
    }

    PAM_AUTH_ERR
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    unsafe { do_authenticate(pamh) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_setcred(
    _pamh: PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    PAM_SUCCESS
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_acct_mgmt(
    _pamh: PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    PAM_SUCCESS
}
