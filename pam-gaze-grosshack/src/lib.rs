// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

#![allow(clippy::missing_safety_doc)]
use pam_gaze_core::*;
use std::os::fd::AsRawFd;
use std::os::raw::{c_char, c_int};
use std::os::unix::thread::JoinHandleExt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

const PROMPT_RETIRE_TIMEOUT: Duration = Duration::from_secs(2);
const PROMPT_SIGNAL_INTERVAL: Duration = Duration::from_millis(50);

async fn authenticate_biometric_with_timeout(
    username: &str,
    service: Option<&str>,
    timeout_duration: Duration,
) -> Option<c_int> {
    let auth_future = authenticate_biometric(username, service);

    tokio::select! {
        res = auth_future => {
            match res {
                Ok(AuthOutcome::Match) => Some(PAM_SUCCESS),
                Ok(AuthOutcome::NoMatch) | Ok(AuthOutcome::Unavailable) => Some(PAM_AUTH_ERR),
                Err(_) => None,
            }
        }
        _ = tokio::time::sleep(timeout_duration) => None,
    }
}

extern "C" fn interrupt_noop_handler(_sig: c_int) {}

/// The handler does nothing; what matters is `sa_flags = 0`, which withholds SA_RESTART so the
/// prompt thread's blocking read fails with EINTR instead of being resumed by the kernel.
fn ensure_interrupt_handler() {
    static INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INSTALLED.get_or_init(|| unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = interrupt_noop_handler as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        action.sa_flags = 0;
        libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut());
    });
}

enum PromptUnblock {
    Injected,
    SignalOnly,
    NoTerminal,
}

fn prompt_is_finished(state: &SharedAuthState) -> bool {
    let (lock, _) = &**state;
    lock.lock().finished
}

fn signal_prompt_until_finished(
    state: &SharedAuthState,
    tid: libc::pthread_t,
    deadline: Duration,
) -> bool {
    ensure_interrupt_handler();
    let start = std::time::Instant::now();

    let (lock, condvar) = &**state;
    let mut shared_state = lock.lock();
    while !shared_state.finished && start.elapsed() < deadline {
        unsafe {
            libc::pthread_kill(tid, libc::SIGUSR1);
        }
        condvar.wait_for(&mut shared_state, PROMPT_SIGNAL_INTERVAL);
    }
    shared_state.finished
}

fn retire_prompt(state: &SharedAuthState, prompt_thread: thread::JoinHandle<()>) {
    let retired = match unblock_terminal() {
        PromptUnblock::Injected => {
            wait_for_prompt_finish(state);
            true
        }
        PromptUnblock::SignalOnly => {
            signal_prompt_until_finished(state, prompt_thread.as_pthread_t(), PROMPT_RETIRE_TIMEOUT)
        }
        PromptUnblock::NoTerminal => prompt_is_finished(state),
    };

    if retired {
        let _ = prompt_thread.join();
    }
}

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let service = unsafe { get_pam_service(pamh) };
    if service_defers_to_face_service(service.as_deref())
        || service_defers_to_face_slot(service.as_deref())
    {
        return PAM_IGNORE;
    }

    // Racing a prompt nobody answers is a deadlock, not a race.
    if service_cannot_be_prompted(service.as_deref()) {
        return PAM_IGNORE;
    }

    let (username, rt) = match unsafe { username_and_runtime(pamh) } {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };

    match enrollment_disposition(rt.block_on(has_enrolled_faces(&username))) {
        EnrollmentDisposition::Ignore => return PAM_IGNORE,
        EnrollmentDisposition::Unavailable => return PAM_AUTHINFO_UNAVAIL,
        EnrollmentDisposition::Continue => {}
    }

    let loaded_auth = rt.block_on(setup_auth_env()).ok().map(|(cfg, _)| cfg.auth);
    let require_confirmation = confirmation_required(loaded_auth.as_ref(), service.as_deref());
    let auth = loaded_auth.unwrap_or_default();

    unsafe { say(pamh, LOOK_OR_PASSWORD_PROMPT) };

    let is_polkit = matches!(service, Some(ref s) if s == "polkit-1");

    let state = new_auth_state();

    let notify = Arc::new(tokio::sync::Notify::new());
    let notify_clone = Arc::clone(&notify);
    let prompt_thread = spawn_prompt_thread(pamh, &state, move || {
        notify_clone.notify_one();
    });

    let biometric_fut = authenticate_biometric_with_timeout(
        &username,
        service.as_deref(),
        camera_auth_timeout(&auth, service.as_deref()),
    );
    let password_fut = notify.notified();

    enum SelectorResult {
        Biometric(Option<c_int>),
        Password,
    }

    let select_res = rt.block_on(async {
        tokio::select! {
            bio_res = biometric_fut => SelectorResult::Biometric(bio_res),
            _ = password_fut => SelectorResult::Password,
        }
    });

    match select_res {
        SelectorResult::Password => {
            let fallback = unsafe { wait_for_password_and_fallback(pamh, &state) };
            let _ = prompt_thread.join();
            fallback
        }
        SelectorResult::Biometric(bio_res) => {
            if bio_res != Some(PAM_SUCCESS) {
                let fallback = unsafe { wait_for_password_and_fallback(pamh, &state) };
                let _ = prompt_thread.join();
                return fallback;
            }

            if !require_confirmation {
                retire_prompt(&state, prompt_thread);
                unsafe { report_face_verified(pamh) };
                return PAM_SUCCESS;
            }

            if !is_polkit {
                retire_prompt(&state, prompt_thread);
                if unsafe { confirm_authentication(pamh) } {
                    PAM_SUCCESS
                } else {
                    PAM_AUTH_ERR
                }
            } else {
                let active_uid = rt.block_on(active_or_user_uid(&username));

                let de = active_uid
                    .map(detect_desktop_environment)
                    .unwrap_or_else(|| "Other".to_string());

                let extension_active =
                    de == "GNOME" && rt.block_on(gnome_extension_active(active_uid));

                unsafe {
                    confirm_graphical_polkit(pamh, &de, extension_active, &state, prompt_thread)
                }
            }
        }
    }
}
/// Push a newline into the tty's input queue to unblock the PAM conversation read. TIOCSTI is
/// compiled out or sysctl-disabled on hardened kernels, so failure falls back to signalling.
fn unblock_terminal() -> PromptUnblock {
    let Ok(tty) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
    else {
        return PromptUnblock::NoTerminal;
    };

    let fd = tty.as_raw_fd();
    let nl = b'\n' as libc::c_char;
    if unsafe { libc::ioctl(fd, libc::TIOCSTI, &nl as *const libc::c_char) == 0 } {
        PromptUnblock::Injected
    } else {
        PromptUnblock::SignalOnly
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    fn mark_finished(state: &SharedAuthState) {
        let (lock, condvar) = &**state;
        lock.lock().finished = true;
        condvar.notify_all();
    }

    fn parked_thread() -> (thread::JoinHandle<()>, mpsc::Sender<()>) {
        let (tx, rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || while rx.recv().is_ok() {});
        (handle, tx)
    }

    #[test]
    fn prompt_is_finished_reads_the_shared_state() {
        let state = new_auth_state();
        assert!(!prompt_is_finished(&state));
        mark_finished(&state);
        assert!(prompt_is_finished(&state));
    }

    #[test]
    fn a_finished_prompt_is_retired_without_signalling() {
        let state = new_auth_state();
        mark_finished(&state);
        let (handle, tx) = parked_thread();

        let start = Instant::now();
        let retired =
            signal_prompt_until_finished(&state, handle.as_pthread_t(), Duration::from_secs(5));

        assert!(retired);
        assert!(
            start.elapsed() < Duration::from_millis(250),
            "should not have waited: {:?}",
            start.elapsed()
        );

        drop(tx);
        let _ = handle.join();
    }

    #[test]
    fn an_unanswerable_prompt_is_abandoned_at_the_deadline() {
        let state = new_auth_state();
        let (handle, tx) = parked_thread();
        let deadline = Duration::from_millis(250);

        let start = Instant::now();
        let retired = signal_prompt_until_finished(&state, handle.as_pthread_t(), deadline);
        let waited = start.elapsed();

        assert!(
            !retired,
            "an unfinished prompt must report that it was abandoned"
        );
        assert!(waited >= deadline, "gave up too early: {waited:?}");
        assert!(
            waited < Duration::from_secs(2),
            "did not honour the deadline: {waited:?}"
        );

        drop(tx);
        let _ = handle.join();
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
