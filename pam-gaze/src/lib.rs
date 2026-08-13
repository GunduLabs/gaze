// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(clippy::missing_safety_doc)]
use gaze_core::dbus::GazeProxy;
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

// Polkit dialogs ignore echo-off confirmation prompts, so keep a password request pending
// for the agent to answer and flip the dialog into confirm mode via the info-message token.
unsafe fn confirm_via_polkit_dialog(
    pamh: PamHandle,
    username: &str,
    proxy: &GazeProxy<'static>,
    rt: &tokio::runtime::Runtime,
) -> c_int {
    let active_uid = rt.block_on(active_or_user_uid(username));
    let de = active_uid
        .map(detect_desktop_environment)
        .unwrap_or_else(|| "Other".to_string());
    let extension_active =
        de == "GNOME" && rt.block_on(gnome_extension_active_on(proxy, active_uid));

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

const RETRY_BACKOFF: Duration = Duration::from_millis(500);

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Reached(AuthOutcome, Option<gaze_core::dbus::CaptureStatus>),
    /// The budget ran out with nothing to report.
    Exhausted,
    /// The daemon could not be reached.
    Failed,
}

type Attempt<E> = Result<(AuthOutcome, Option<gaze_core::dbus::CaptureStatus>), E>;

/// Darkness will still be dark in half a second; an empty frame may not be.
fn worth_another_look(status: Option<gaze_core::dbus::CaptureStatus>) -> bool {
    !matches!(status, Some(gaze_core::dbus::CaptureStatus::TooDark))
}

async fn verify_within(
    proxy: &GazeProxy<'static>,
    username: &str,
    service: Option<&str>,
    budget: Duration,
) -> Verdict {
    let verdict = verify_until(budget, service_retries_transient_give_up(service), || {
        authenticate_biometric_with_status_on(proxy, username, service)
    })
    .await;

    // Running out of budget drops the attempt mid-flight, and its release guard would spawn
    // onto a runtime this call is about to drop. Give the daemon its camera back explicitly.
    if !matches!(verdict, Verdict::Reached(AuthOutcome::Match, _)) {
        let _ = proxy.release().await;
    }
    verdict
}

async fn verify_until<F, Fut, E>(budget: Duration, retry: bool, mut attempt: F) -> Verdict
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Attempt<E>>,
{
    let deadline = tokio::time::Instant::now() + budget;
    let left = |deadline: tokio::time::Instant| {
        deadline.saturating_duration_since(tokio::time::Instant::now())
    };
    let mut given_up = None;
    // "No face detected" beats a bare timeout.
    let expired = |given_up: Option<_>| match given_up {
        Some(status) => Verdict::Reached(AuthOutcome::Unavailable, status),
        None => Verdict::Exhausted,
    };

    loop {
        let remaining = left(deadline);
        if remaining.is_zero() {
            return expired(given_up);
        }

        match timeout(remaining, attempt()).await {
            Ok(Ok((AuthOutcome::Unavailable, status))) if retry && worth_another_look(status) => {
                given_up = Some(status);
                // Saturating: underflow would panic across the PAM FFI boundary.
                tokio::time::sleep(RETRY_BACKOFF.min(left(deadline))).await;
            }
            Ok(Ok((outcome, status))) => return Verdict::Reached(outcome, status),
            Ok(Err(_)) => return Verdict::Failed,
            Err(_) => return expired(given_up),
        }
    }
}

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let service = unsafe { get_pam_service(pamh) };
    if service_defers_to_face_service(service.as_deref())
        || service_defers_to_face_slot(service.as_deref())
    {
        return PAM_IGNORE;
    }

    let (username, rt) = match unsafe { username_and_runtime(pamh) } {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };

    let is_polkit = matches!(service, Some(ref s) if s == "polkit-1");

    let matched = rt.block_on(async {
        let (config, proxy) = setup_auth_env().await.map_err(|_| PAM_AUTHINFO_UNAVAIL)?;

        match enrollment_disposition(has_enrolled_faces_on(&proxy, &username).await) {
            EnrollmentDisposition::Ignore => return Err(PAM_IGNORE),
            EnrollmentDisposition::Unavailable => return Err(PAM_AUTHINFO_UNAVAIL),
            EnrollmentDisposition::Continue => {}
        }

        let prompt = if is_polkit {
            LOOK_OR_PASSWORD_PROMPT
        } else {
            LOOK_PROMPT
        };
        // KScreenLocker reads an info message as "this unlock had a prompt".
        unsafe { say(pamh, prompt) };

        let budget = camera_auth_timeout(&config.auth, service.as_deref());

        let tell = |text: &str| unsafe { report(pamh, service.as_deref(), text) };

        let verdict = verify_within(&proxy, &username, service.as_deref(), budget).await;
        match verdict {
            Verdict::Reached(AuthOutcome::Match, _) => Ok((config.auth, proxy)),
            Verdict::Reached(AuthOutcome::NoMatch, _) => {
                tell(FACE_NOT_RECOGNIZED);
                Err(PAM_AUTH_ERR)
            }
            Verdict::Reached(AuthOutcome::Unavailable, status) => {
                tell(give_up_message(status));
                Err(PAM_AUTHINFO_UNAVAIL)
            }
            Verdict::Exhausted => {
                tell(FACE_TIMED_OUT);
                Err(PAM_AUTHINFO_UNAVAIL)
            }
            Verdict::Failed => {
                tell(FACE_UNAVAILABLE);
                Err(PAM_AUTHINFO_UNAVAIL)
            }
        }
    });
    let (loaded_auth, proxy) = match matched {
        Ok(session) => session,
        Err(code) => return code,
    };

    if !confirmation_required(Some(&loaded_auth), service.as_deref()) {
        unsafe { report_face_verified(pamh) };
        return PAM_SUCCESS;
    }

    // A prompt on a slot nobody answers blocks until the lock ends.
    if service_cannot_be_prompted(service.as_deref()) {
        return PAM_SUCCESS;
    }

    if is_polkit {
        return unsafe { confirm_via_polkit_dialog(pamh, &username, &proxy, &rt) };
    }

    if has_controlling_tty() {
        return if unsafe { confirm_authentication(pamh) } {
            PAM_SUCCESS
        } else {
            PAM_AUTH_ERR
        };
    }

    let (uid, is_greeter) = rt.block_on(active_confirm_target(&username));
    let de = uid
        .map(detect_desktop_environment)
        .unwrap_or_else(|| "Other".to_string());
    // The greeter always runs GNOME + the extension, so query it directly rather than trusting
    // DE detection on transient processes; otherwise GDM silently bypasses Require Confirmation.
    let extension_active =
        (is_greeter || de == "GNOME") && rt.block_on(gnome_extension_active_on(&proxy, uid));

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

#[cfg(test)]
mod tests {
    use super::*;
    use gaze_core::dbus::CaptureStatus;
    use std::cell::Cell;

    type Unreachable = &'static str;

    fn scripted(
        script: Vec<Attempt<Unreachable>>,
    ) -> impl FnMut() -> std::future::Ready<Attempt<Unreachable>> {
        let calls = Cell::new(0usize);
        move || {
            let index = calls.get().min(script.len() - 1);
            calls.set(calls.get() + 1);
            std::future::ready(script[index])
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_match_is_returned_without_retrying() {
        let verdict = verify_until(
            Duration::from_secs(12),
            true,
            scripted(vec![Ok((AuthOutcome::Match, None))]),
        )
        .await;
        assert_eq!(verdict, Verdict::Reached(AuthOutcome::Match, None));
    }

    #[tokio::test(start_paused = true)]
    async fn a_no_match_is_final_even_where_retrying_is_enabled() {
        let verdict = verify_until(
            Duration::from_secs(12),
            true,
            scripted(vec![
                Ok((AuthOutcome::NoMatch, None)),
                Ok((AuthOutcome::Match, None)),
            ]),
        )
        .await;
        assert_eq!(verdict, Verdict::Reached(AuthOutcome::NoMatch, None));
    }

    #[tokio::test(start_paused = true)]
    async fn an_empty_frame_is_retried_until_a_face_arrives() {
        let verdict = verify_until(
            Duration::from_secs(12),
            true,
            scripted(vec![
                Ok((AuthOutcome::Unavailable, Some(CaptureStatus::NoFace))),
                Ok((AuthOutcome::Unavailable, Some(CaptureStatus::NoFace))),
                Ok((AuthOutcome::Match, None)),
            ]),
        )
        .await;
        assert_eq!(verdict, Verdict::Reached(AuthOutcome::Match, None));
    }

    #[tokio::test(start_paused = true)]
    async fn darkness_is_not_retried() {
        let verdict = verify_until(
            Duration::from_secs(12),
            true,
            scripted(vec![
                Ok((AuthOutcome::Unavailable, Some(CaptureStatus::TooDark))),
                Ok((AuthOutcome::Match, None)),
            ]),
        )
        .await;
        assert_eq!(
            verdict,
            Verdict::Reached(AuthOutcome::Unavailable, Some(CaptureStatus::TooDark))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_give_up_is_returned_once_without_retrying() {
        let verdict = verify_until(
            Duration::from_secs(12),
            false,
            scripted(vec![
                Ok((AuthOutcome::Unavailable, Some(CaptureStatus::NoFace))),
                Ok((AuthOutcome::Match, None)),
            ]),
        )
        .await;
        assert_eq!(
            verdict,
            Verdict::Reached(AuthOutcome::Unavailable, Some(CaptureStatus::NoFace))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn retrying_stops_at_the_budget_and_reports_the_last_reason() {
        let verdict = verify_until(
            Duration::from_secs(12),
            true,
            scripted(vec![Ok((
                AuthOutcome::Unavailable,
                Some(CaptureStatus::NoFace),
            ))]),
        )
        .await;
        assert_eq!(
            verdict,
            Verdict::Reached(AuthOutcome::Unavailable, Some(CaptureStatus::NoFace))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_scan_that_never_returns_is_reported_as_exhausted() {
        let verdict = verify_until(Duration::from_secs(12), true, || {
            std::future::pending::<Attempt<Unreachable>>()
        })
        .await;
        assert_eq!(verdict, Verdict::Exhausted);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unreachable_daemon_is_not_retried() {
        let verdict =
            verify_until(Duration::from_secs(12), true, scripted(vec![Err("no bus")])).await;
        assert_eq!(verdict, Verdict::Failed);
    }

    #[tokio::test(start_paused = true)]
    async fn a_zero_budget_does_not_scan_at_all() {
        let verdict = verify_until(
            Duration::ZERO,
            true,
            || -> std::future::Ready<Attempt<Unreachable>> {
                panic!("must not attempt verification with no budget")
            },
        )
        .await;
        assert_eq!(verdict, Verdict::Exhausted);
    }
}
