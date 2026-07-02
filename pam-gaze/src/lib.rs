#![allow(clippy::missing_safety_doc)]
use pam_gaze_core::*;
use std::os::raw::{c_char, c_int};
use std::time::Duration;
use tokio::time::timeout;

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let (username, rt) = match unsafe { username_and_runtime(pamh) } {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };

    rt.block_on(async {
        match enrollment_disposition(has_enrolled_faces(&username).await) {
            EnrollmentDisposition::Ignore => return PAM_IGNORE,
            EnrollmentDisposition::Unavailable => return PAM_AUTHINFO_UNAVAIL,
            EnrollmentDisposition::Continue => {}
        }

        unsafe { say(pamh, "Please look at the camera") };

        match timeout(
            Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS),
            authenticate_biometric(&username),
        )
        .await
        {
            Ok(Ok(AuthOutcome::Match)) => PAM_SUCCESS,
            Ok(Ok(AuthOutcome::NoMatch)) => PAM_AUTH_ERR,
            Ok(Ok(AuthOutcome::Unavailable)) => PAM_AUTHINFO_UNAVAIL,
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

pam_gaze_core::pam_success_stubs!();
