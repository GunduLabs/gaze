// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

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
    let service = unsafe { get_pam_service(pamh) };
    if service_defers_to_face_service(service.as_deref()) {
        return PAM_IGNORE;
    }

    let (username, rt) = match unsafe { username_and_runtime(pamh) } {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };

    let is_polkit = matches!(service, Some(ref s) if s == "polkit-1");

    let matched = rt.block_on(async {
        match enrollment_disposition(has_enrolled_faces(&username).await) {
            EnrollmentDisposition::Ignore => return Err(PAM_IGNORE),
            EnrollmentDisposition::Unavailable => return Err(PAM_AUTHINFO_UNAVAIL),
            EnrollmentDisposition::Continue => {}
        }

        let prompt = if is_polkit {
            LOOK_OR_PASSWORD_PROMPT
        } else {
            LOOK_PROMPT
        };
        unsafe { say(pamh, prompt) };

        let loaded_auth = setup_auth_env().await.ok().map(|(config, _)| config.auth);
        let budget = match loaded_auth.as_ref() {
            Some(auth) => camera_auth_timeout(auth, service.as_deref()),
            None => Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS),
        };

        match timeout(
            budget,
            authenticate_biometric_with_status(&username, service.as_deref()),
        )
        .await
        {
            Ok(Ok((AuthOutcome::Match, _))) => Ok(loaded_auth),
            Ok(Ok((AuthOutcome::NoMatch, _))) => {
                unsafe { say(pamh, FACE_NOT_RECOGNIZED) };
                Err(PAM_AUTH_ERR)
            }
            Ok(Ok((AuthOutcome::Unavailable, status))) => {
                unsafe { say(pamh, give_up_message(status)) };
                Err(PAM_AUTHINFO_UNAVAIL)
            }
            Ok(Err(_)) => {
                unsafe { say(pamh, FACE_UNAVAILABLE) };
                Err(PAM_AUTHINFO_UNAVAIL)
            }
            Err(_) => {
                unsafe { say(pamh, FACE_TIMED_OUT) };
                Err(PAM_AUTHINFO_UNAVAIL)
            }
        }
    });
    let loaded_auth = match matched {
        Ok(auth) => auth,
        Err(code) => return code,
    };

    if !confirmation_required(loaded_auth.as_ref()) {
        return PAM_SUCCESS;
    }

    if has_controlling_tty() {
        return if unsafe { confirm_authentication(pamh) } {
            PAM_SUCCESS
        } else {
            PAM_AUTH_ERR
        };
    }

    if is_polkit {
        return unsafe { confirm_via_polkit_dialog(pamh, &username, &rt) };
    }

    let (uid, is_greeter) = rt.block_on(active_confirm_target(&username));
    let de = uid
        .map(detect_desktop_environment)
        .unwrap_or_else(|| "Other".to_string());
    // The greeter always runs GNOME + the gaze extension, so query the
    // extension directly rather than trusting DE detection on its transient
    // processes; otherwise GDM silently bypasses Require Confirmation.
    let extension_active =
        (is_greeter || de == "GNOME") && rt.block_on(gnome_extension_active(uid));

    match graphical_confirm_decision(&de, extension_active, is_greeter) {
        GraphicalConfirm::GnomeExtension => confirm_via_gnome_extension(pamh),
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
