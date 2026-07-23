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

// Polkit dialogs ignore echo-off confirmation prompts, so keep a password
// request pending for the agent to answer, then flip the dialog into
// confirm mode via the info-message token.
unsafe fn confirm_via_polkit_dialog(
    pamh: PamHandle,
    username: &str,
    rt: &tokio::runtime::Runtime,
) -> c_int {
    let active_uid = rt.block_on(active_or_user_uid(username));
    let de = active_uid
        .map(detect_desktop_environment)
        .unwrap_or_else(|| "Other".to_string());
    let extension_active = de == "GNOME" && rt.block_on(gnome_extension_active(active_uid));

    // No confirm channel without the extension; let the stack fall
    // through to password auth.
    if de == "GNOME" && !extension_active {
        return PAM_AUTH_ERR;
    }

    let state = new_auth_state();
    let prompt_thread = spawn_prompt_thread(pamh, &state, || {});
    wait_for_prompt_started(&state);
    // Let the pending request reach the dialog before the confirm token,
    // or the dialog re-shows the password entry.
    std::thread::sleep(Duration::from_millis(150));

    unsafe { confirm_graphical_polkit(pamh, &de, extension_active, &state, prompt_thread) }
}

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let (username, rt) = match unsafe { username_and_runtime(pamh) } {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };

    let live_status = service_wants_live_status(unsafe { get_pam_service(pamh) }.as_deref());

    let matched = rt.block_on(async {
        match enrollment_disposition(has_enrolled_faces(&username).await) {
            EnrollmentDisposition::Ignore => return Err(PAM_IGNORE),
            EnrollmentDisposition::Unavailable => return Err(PAM_AUTHINFO_UNAVAIL),
            EnrollmentDisposition::Continue => {}
        }

        unsafe { say(pamh, "Please look at the camera") };

        let biometric = authenticate_biometric_with_status(&username, |status| {
            if live_status {
                unsafe { say(pamh, &status.to_string()) };
            }
        });

        match timeout(Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS), biometric).await {
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

    if matches!(unsafe { get_pam_service(pamh) }, Some(ref s) if s == "polkit-1") {
        return unsafe { confirm_via_polkit_dialog(pamh, &username, &rt) };
    }

    let uid = rt.block_on(active_or_user_uid(&username));
    let de = uid
        .map(detect_desktop_environment)
        .unwrap_or_else(|| "Other".to_string());
    let extension_active = de == "GNOME" && rt.block_on(gnome_extension_active(uid));

    match graphical_confirm_decision(&de, extension_active) {
        GraphicalConfirm::GnomeExtension => confirm_via_gnome_extension(pamh),
        GraphicalConfirm::Bypass => PAM_SUCCESS,
        GraphicalConfirm::FailClosed => PAM_AUTH_ERR,
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

pam_gaze_core::pam_success_stubs!();
