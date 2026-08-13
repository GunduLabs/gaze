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

#[inline(never)]
fn interrupt_handler_address() -> usize {
    interrupt_noop_handler as *const () as usize
}

/// Borrows SIGUSR1 while a prompt is being interrupted, and gives the host process it runs
/// inside its own back. `sa_flags = 0` withholds SA_RESTART, so the blocked read sees EINTR.
struct InterruptHandler {
    previous: libc::sigaction,
}

impl InterruptHandler {
    fn install() -> Self {
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = interrupt_handler_address();
            libc::sigemptyset(&mut action.sa_mask);
            action.sa_flags = 0;
            let mut previous: libc::sigaction = std::mem::zeroed();
            libc::sigaction(libc::SIGUSR1, &action, &mut previous);
            Self { previous }
        }
    }
}

impl Drop for InterruptHandler {
    fn drop(&mut self) {
        unsafe {
            libc::sigaction(libc::SIGUSR1, &self.previous, std::ptr::null_mut());
        }
    }
}

enum PromptUnblock {
    Injected,
    SignalOnly,
    NoTerminal,
}

/// The caller must hold an [`InterruptHandler`] until the thread is joined: at SIGUSR1's
/// default disposition, a signal still in flight would kill the host process.
fn signal_prompt_until_finished(
    state: &SharedAuthState,
    tid: libc::pthread_t,
    deadline: Duration,
) -> bool {
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

/// Bounded, because an injected newline only helps a conversation that reads the terminal. A
/// graphical agent reads its own socket and would never see it.
fn wait_for_prompt_finish_within(state: &SharedAuthState, deadline: Duration) -> bool {
    let start = std::time::Instant::now();
    let (lock, condvar) = &**state;
    let mut shared_state = lock.lock();
    while !shared_state.finished {
        let Some(left) = deadline.checked_sub(start.elapsed()) else {
            break;
        };
        condvar.wait_for(&mut shared_state, left);
    }
    shared_state.finished
}

/// Whether a prompt started now could be unblocked again, rather than parking a thread inside
/// the caller's conversation with no way back out.
fn prompt_is_retirable() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
}

fn prompt_is_finished(state: &SharedAuthState) -> bool {
    let (lock, _) = &**state;
    lock.lock().finished
}

fn retire_prompt(state: &SharedAuthState, prompt_thread: thread::JoinHandle<()>) {
    let tid = prompt_thread.as_pthread_t();
    // Held past the join, so no signal outlives the handler.
    let _interrupts = InterruptHandler::install();

    // Nothing to unblock if the user already answered, and an injected newline would be left
    // for the confirmation prompt, which takes a bare newline as the confirmation.
    let mut retired = prompt_is_finished(state);

    if !retired {
        retired = match unblock_terminal() {
            PromptUnblock::Injected => wait_for_prompt_finish_within(state, PROMPT_RETIRE_TIMEOUT),
            // Signalling interrupts the blocking read itself, so it needs no terminal and is the
            // only lever left when there is none.
            PromptUnblock::SignalOnly | PromptUnblock::NoTerminal => {
                signal_prompt_until_finished(state, tid, PROMPT_RETIRE_TIMEOUT)
            }
        };
    }

    // The thread holds the handle `pam_end` is about to free, so it cannot be abandoned. Keep
    // interrupting rather than blocking in `join` with nothing left trying to wake it.
    while !retired {
        retired = signal_prompt_until_finished(state, tid, PROMPT_RETIRE_TIMEOUT);
    }

    let _ = prompt_thread.join();
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

    // Without a terminal a prompt can only be retired by signalling, and graphical conversations
    // resume their own read on EINTR. Polkit is exempt: it consumes the prompt for confirmation.
    if !prompt_is_retirable() && !is_polkit {
        let bio = rt.block_on(authenticate_biometric_with_timeout(
            &username,
            service.as_deref(),
            camera_auth_timeout(&auth, service.as_deref()),
        ));
        if bio != Some(PAM_SUCCESS) {
            return PAM_AUTHINFO_UNAVAIL;
        }
        if require_confirmation {
            // Its own conversation, on this thread, so there is nothing left to unblock.
            return if unsafe { confirm_authentication(pamh) } {
                PAM_SUCCESS
            } else {
                PAM_AUTH_ERR
            };
        }
        unsafe { report_face_verified(pamh) };
        return PAM_SUCCESS;
    }

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
    fn a_finished_prompt_is_retired_without_signalling() {
        let state = new_auth_state();
        mark_finished(&state);
        let (handle, tx) = parked_thread();

        let _interrupts = InterruptHandler::install();
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

        let _interrupts = InterruptHandler::install();
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

    #[test]
    fn the_hosts_signal_disposition_is_given_back() {
        // This runs inside sudo, login, or a display manager. Leaving SIGUSR1 pointed at our
        // no-op handler would silently disarm whatever the host uses it for.
        extern "C" fn host_handler(_sig: c_int) {}

        let mut host: libc::sigaction = unsafe { std::mem::zeroed() };
        host.sa_sigaction = host_handler as *const () as usize;
        let mut original: libc::sigaction = unsafe { std::mem::zeroed() };
        unsafe { libc::sigaction(libc::SIGUSR1, &host, &mut original) };

        {
            let _interrupts = InterruptHandler::install();
            let mut during: libc::sigaction = unsafe { std::mem::zeroed() };
            unsafe { libc::sigaction(libc::SIGUSR1, std::ptr::null(), &mut during) };
            assert_eq!(
                during.sa_sigaction,
                interrupt_handler_address(),
                "the interrupting handler must be in place while signalling"
            );
        }

        let mut after: libc::sigaction = unsafe { std::mem::zeroed() };
        unsafe { libc::sigaction(libc::SIGUSR1, std::ptr::null(), &mut after) };
        assert_eq!(
            after.sa_sigaction, host.sa_sigaction,
            "the host's handler must be back"
        );

        unsafe { libc::sigaction(libc::SIGUSR1, &original, std::ptr::null_mut()) };
    }

    #[test]
    fn an_answered_prompt_is_retired_without_touching_the_terminal() {
        let state = new_auth_state();
        mark_finished(&state);
        let (handle, tx) = parked_thread();

        assert!(prompt_is_finished(&state));
        drop(tx);
        retire_prompt(&state, handle);
    }

    #[test]
    fn waiting_for_an_injected_newline_gives_up_at_the_deadline() {
        // A conversation that never reads the terminal keeps `finished` false. The wait has to
        // return anyway, or `retire_prompt` would park here instead of signalling.
        let state = new_auth_state();
        let deadline = Duration::from_millis(250);

        let start = Instant::now();
        let finished = wait_for_prompt_finish_within(&state, deadline);
        let waited = start.elapsed();

        assert!(!finished);
        assert!(waited >= deadline, "returned too early: {waited:?}");
        assert!(
            waited < Duration::from_secs(2),
            "did not honour the deadline: {waited:?}"
        );

        mark_finished(&state);
        let start = Instant::now();
        assert!(wait_for_prompt_finish_within(&state, deadline));
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "a finished prompt should not be waited on"
        );
    }

    #[test]
    fn a_prompt_that_finishes_late_is_still_retired() {
        // The unblock attempt times out, then the conversation returns. `retire_prompt` must
        // notice and join rather than keep signalling.
        let state = new_auth_state();
        let (handle, tx) = parked_thread();

        let waker = {
            let state = state.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                mark_finished(&state);
            })
        };

        // Let the thread itself exit; `retire_prompt` still has to notice `finished` before it
        // stops signalling, which is what this exercises.
        drop(tx);
        retire_prompt(&state, handle);
        let _ = waker.join();
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
