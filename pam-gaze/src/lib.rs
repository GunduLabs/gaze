#![allow(clippy::missing_safety_doc)]
use pam_gaze_core::*;
use std::os::raw::{c_char, c_int};
use std::time::Duration;
use tokio::time::timeout;

fn confirm_via_gnome_extension(pamh: PamHandle) -> c_int {
    let response = unsafe { converse(pamh, PAM_PROMPT_ECHO_OFF, CONFIRMATION_REQUEST) };
    if confirmation_accepted(response.as_deref()) {
        PAM_SUCCESS
    } else {
        PAM_AUTH_ERR
    }
}

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let (username, rt) = match unsafe { username_and_runtime(pamh) } {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };

    let matched = rt.block_on(async {
        match enrollment_disposition(has_enrolled_faces(&username).await) {
            EnrollmentDisposition::Ignore => return Err(PAM_IGNORE),
            EnrollmentDisposition::Unavailable => return Err(PAM_AUTHINFO_UNAVAIL),
            EnrollmentDisposition::Continue => {}
        }

        unsafe { say(pamh, "Please look at the camera") };

        match timeout(
            Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS),
            authenticate_biometric(&username),
        )
        .await
        {
            Ok(Ok(AuthOutcome::Match)) => Ok(()),
            Ok(Ok(AuthOutcome::NoMatch)) => Err(PAM_AUTH_ERR),
            Ok(Ok(AuthOutcome::Unavailable)) => Err(PAM_AUTHINFO_UNAVAIL),
            _ => Err(PAM_AUTHINFO_UNAVAIL),
        }
    });
    if let Err(code) = matched {
        return code;
    }

    let require_confirmation = rt.block_on(async {
        match setup_auth_env().await {
            Ok((config, _)) => config.auth.require_confirmation,
            Err(_) => false,
        }
    });
    if !require_confirmation {
        return PAM_SUCCESS;
    }

    if has_controlling_tty() {
        return if unsafe { confirm_authentication(pamh) } {
            PAM_SUCCESS
        } else {
            PAM_AUTH_ERR
        };
    }

    let extension_active = rt.block_on(async {
        let uid = active_or_user_uid(&username).await;
        gnome_extension_active(uid).await
    });
    // No confirmation channel (no TTY, no GNOME extension): fail closed rather
    // than granting on the face match alone, or require_confirmation is a no-op.
    if !extension_active {
        return PAM_AUTH_ERR;
    }

    confirm_via_gnome_extension(pamh)
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
