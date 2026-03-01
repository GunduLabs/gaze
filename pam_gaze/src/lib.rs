#![allow(clippy::missing_safety_doc)]
use gaze_core::capture::{init_camera_and_checker, wait_for_capture};
use pam_gaze_core::*;
use std::os::raw::{c_char, c_int};

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let Some(username) = (unsafe { get_username(pamh) }) else {
        return PAM_AUTH_ERR;
    };

    let Ok((config, _, proxy)) = setup_auth_env() else {
        unsafe { say(pamh, "Face authentication unavailable") };
        return PAM_AUTHINFO_UNAVAIL;
    };
    let Ok((mut cam, mut checker)) = init_camera_and_checker(&config.cameras.rgb) else {
        return PAM_SERVICE_ERR;
    };

    unsafe { say(pamh, "Please look at the camera") };

    let mut last_hint: Option<String> = None;

    let capture = {
        let Ok(capture) = wait_for_capture(&mut cam, &mut checker, false, |status| {
            let hint = status.to_string();
            if last_hint.as_deref() != Some(hint.as_str()) {
                unsafe { say(pamh, &hint) };
                last_hint = Some(hint);
            }
        }) else {
            return PAM_SERVICE_ERR;
        };
        capture
    };

    match proxy.verify(&username, &capture.bytes, capture.width, capture.height) {
        Ok(true) => PAM_SUCCESS,
        Ok(false) => PAM_AUTH_ERR,
        Err(_) => PAM_SERVICE_ERR,
    }
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
