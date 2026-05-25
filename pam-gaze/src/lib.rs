#![allow(clippy::missing_safety_doc)]
use pam_gaze_core::*;
use std::os::raw::{c_char, c_int};
use std::time::Duration;
use tokio::time::timeout;

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let Some(username) = (unsafe { get_username(pamh) }) else {
        return PAM_AUTH_ERR;
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(_) => return PAM_AUTHINFO_UNAVAIL,
    };

    rt.block_on(async {
        if let Ok(false) = has_enrolled_faces(&username).await {
            return PAM_IGNORE;
        }

        unsafe { say(pamh, "Please look at the camera") };

        match timeout(
            Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS),
            authenticate_biometric(&username),
        )
        .await
        {
            Ok(Ok(Some(true))) => PAM_SUCCESS,
            Ok(Ok(Some(false))) => PAM_AUTH_ERR,
            Ok(Ok(None)) => PAM_IGNORE,
            _ => PAM_AUTHINFO_UNAVAIL,
        }
    })
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
