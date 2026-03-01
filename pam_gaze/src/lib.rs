#![allow(clippy::missing_safety_doc)]
use gaze_core::capture::frame_to_bytes;
use pam_gaze_core::*;
use std::os::raw::{c_char, c_int};

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let Some(username) = (unsafe { get_username(pamh) }) else {
        return PAM_AUTH_ERR;
    };

    let Ok((_, mut cam, proxy)) = setup_auth_env() else {
        unsafe { say(pamh, "Face authentication unavailable") };
        return PAM_AUTHINFO_UNAVAIL;
    };

    unsafe { say(pamh, "Please look at the camera") };

    for _ in 0..MAX_ATTEMPTS {
        let Ok(frame) = cam.capture_frame() else {
            continue;
        };
        let Ok(capture) = frame_to_bytes(&frame) else {
            continue;
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
