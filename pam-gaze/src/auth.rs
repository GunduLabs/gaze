// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::core::*;
use gaze_core::dbus::GazeProxy;
use std::ffi::CStr;
use std::os::fd::AsRawFd;
use std::os::raw::{c_char, c_int};
use std::os::unix::thread::JoinHandleExt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::time::timeout;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PamMode {
    #[default]
    Sequential,
    Simultaneous,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PamOptions {
    pub mode: PamMode,
    pub kde_login: bool,
}

pub fn parse_pam_options<'a, I>(args: I) -> PamOptions
where
    I: IntoIterator<Item = &'a str>,
{
    let mut options = PamOptions::default();
    for arg in args {
        match arg {
            "simultaneous" => options.mode = PamMode::Simultaneous,
            "retry" => options.mode = PamMode::Retry,
            "kde-login" => options.kde_login = true,
            _ => {}
        }
    }
    options
}

pub fn parse_pam_mode<'a, I>(args: I) -> PamMode
where
    I: IntoIterator<Item = &'a str>,
{
    parse_pam_options(args).mode
}

pub unsafe fn parse_raw_pam_options(argc: c_int, argv: *const *const c_char) -> PamOptions {
    if argc <= 0 || argv.is_null() {
        return PamOptions::default();
    }
    let mut options = PamOptions::default();
    for i in 0..argc as isize {
        let arg_ptr = unsafe { *argv.offset(i) };
        if arg_ptr.is_null() {
            continue;
        }
        match unsafe { CStr::from_ptr(arg_ptr) }.to_str() {
            Ok("simultaneous") => options.mode = PamMode::Simultaneous,
            Ok("retry") => options.mode = PamMode::Retry,
            Ok("kde-login") => options.kde_login = true,
            _ => {}
        }
    }
    options
}

// Polkit dialogs ignore echo-off confirmation prompts, so keep a password request pending
// for the agent to answer and flip the dialog into confirm mode via the info-message token.
unsafe fn confirm_via_polkit_dialog(pamh: PamHandle, is_internal: bool) -> c_int {
    let state = new_auth_state();
    let prompt_thread = spawn_prompt_thread(pamh, &state, || {});
    wait_for_prompt_started(&state);
    // Let the pending request reach the dialog before the confirm token,
    // or the dialog re-shows the password entry.
    std::thread::sleep(Duration::from_millis(150));

    unsafe { confirm_graphical_polkit(pamh, &state, prompt_thread, is_internal) }
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
    require_keyring: bool,
) -> Verdict {
    let verdict = verify_until(budget, service_retries_transient_give_up(service), || {
        authenticate_biometric_with_status_on(proxy, username, service, require_keyring)
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

fn retry_is_warranted(verdict: Option<FirstPassVerdict>) -> bool {
    verdict.is_none_or(FirstPassVerdict::allows_retry)
}

fn sequential_handoff(verdict: &Verdict) -> Option<FirstPassVerdict> {
    // Only an actual non-match suppresses the later retry PAM entry. Timeouts and unavailable
    // cameras leave identity undecided, so that entry may try again after password fallback.
    match verdict {
        Verdict::Reached(AuthOutcome::Match, _) => None,
        Verdict::Reached(AuthOutcome::NoMatch, _) => Some(FirstPassVerdict::NoMatch),
        _ => Some(FirstPassVerdict::Undecided),
    }
}

unsafe fn do_authenticate_sequential(pamh: PamHandle, flags: c_int, options: PamOptions) -> c_int {
    let service = unsafe { get_pam_service(pamh) };
    if service_defers_to_face_service(service.as_deref())
        || service_defers_to_face_slot(service.as_deref())
    {
        return PAM_IGNORE;
    }

    let is_retry = options.mode == PamMode::Retry;
    if is_retry && !retry_is_warranted(unsafe { read_first_pass_verdict(pamh) }) {
        return PAM_AUTHINFO_UNAVAIL;
    }

    let silent = caller_wants_silence(flags);

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

        let pam_internal = gaze_core::dbus::get_pam_internal(&proxy).await;
        let is_internal = service
            .as_deref()
            .is_some_and(|s| is_service_internal(s, &pam_internal));

        let prompt = if is_internal {
            if is_polkit && !is_retry {
                GAZE_MSG_LOOK_OR_PASSWORD
            } else {
                GAZE_MSG_LOOK_CAMERA
            }
        } else if is_retry {
            LOOK_AFTER_PASSWORD_PROMPT
        } else if is_polkit {
            LOOK_OR_PASSWORD_PROMPT
        } else {
            LOOK_PROMPT
        };
        // KScreenLocker reads an info message as "this unlock had a prompt".
        let prompt_line = unsafe { announce_prompt(pamh, silent, prompt, is_internal) };

        let budget = camera_auth_timeout(&config.auth, service.as_deref());

        let tell = |text: &str| {
            unsafe { report_outcome(pamh, service.as_deref(), silent, text, is_internal) };
        };

        let require_keyring = keyring_backend(service.as_deref(), &config).is_some();
        let verdict = verify_within(
            &proxy,
            &username,
            service.as_deref(),
            budget,
            require_keyring,
        )
        .await;
        if let Some(handoff) = sequential_handoff(&verdict) {
            unsafe { record_first_pass_verdict(pamh, handoff) };
        }
        match verdict {
            Verdict::Reached(AuthOutcome::Match, _) => Ok((config, is_internal, prompt_line)),
            Verdict::Reached(AuthOutcome::NoMatch, _) => {
                tell(if is_internal {
                    GAZE_MSG_FACE_NOT_RECOGNIZED
                } else {
                    FACE_NOT_RECOGNIZED
                });
                Err(PAM_AUTH_ERR)
            }
            Verdict::Reached(AuthOutcome::Unavailable, status) => {
                tell(if is_internal {
                    internal_give_up_message(status)
                } else {
                    give_up_message(status)
                });
                Err(PAM_AUTHINFO_UNAVAIL)
            }
            Verdict::Exhausted => {
                tell(if is_internal {
                    GAZE_MSG_FACE_TIMED_OUT
                } else {
                    FACE_TIMED_OUT
                });
                Err(PAM_AUTHINFO_UNAVAIL)
            }
            Verdict::Failed => {
                tell(if is_internal {
                    GAZE_MSG_FACE_UNAVAILABLE
                } else {
                    FACE_UNAVAILABLE
                });
                Err(PAM_AUTHINFO_UNAVAIL)
            }
        }
    });
    let (loaded_config, is_internal, prompt_line) = match matched {
        Ok(session) => session,
        Err(code) => return code,
    };

    let authenticated = if !confirmation_required(Some(&loaded_config.auth), service.as_deref()) {
        unsafe { report_face_verified(pamh, silent, prompt_line, is_internal) };
        PAM_SUCCESS
    } else if service_cannot_be_prompted(service.as_deref()) {
        // A prompt on a slot nobody answers blocks until the lock ends.
        PAM_SUCCESS
    } else if is_polkit {
        unsafe { confirm_via_polkit_dialog(pamh, is_internal) }
    } else {
        let confirmed = if is_internal {
            unsafe { confirm_authentication_internal(pamh) }
        } else {
            unsafe { confirm_authentication(pamh, prompt_line) }
        };
        if confirmed { PAM_SUCCESS } else { PAM_AUTH_ERR }
    };
    let result = finish_keyring(
        authenticated,
        service.as_deref(),
        &loaded_config,
        || unsafe {
            supply_keyring_token(
                pamh,
                &username,
                keyring_backend(service.as_deref(), &loaded_config).unwrap(),
            )
        },
    );
    if authenticated == PAM_SUCCESS && result == PAM_AUTHINFO_UNAVAIL {
        unsafe {
            report_outcome(
                pamh,
                service.as_deref(),
                silent,
                "Keyring unlock unavailable. Enter your password.",
                is_internal,
            )
        };
    }
    // pam_kwallet5 prompts when PAM_AUTHTOK is null, but ignores an empty token.
    // Successful unenrolled/opted-out face logins must not create a second greeter prompt.
    if result == PAM_SUCCESS && is_kwallet_login(service.as_deref()) {
        let mut existing = std::ptr::null();
        if unsafe { pam_get_item(pamh, PAM_AUTHTOK, &mut existing) } != PAM_SUCCESS {
            return PAM_AUTHINFO_UNAVAIL;
        }
        if existing.is_null()
            && unsafe { pam_set_item(pamh, PAM_AUTHTOK, c"".as_ptr().cast()) } != PAM_SUCCESS
        {
            return PAM_AUTHINFO_UNAVAIL;
        }
    }
    result
}

fn is_kwallet_login(service: Option<&str>) -> bool {
    matches!(
        service,
        Some("sddm" | "plasmalogin" | "plasmalogin-fingerprint")
    )
}

fn keyring_backend(
    service: Option<&str>,
    config: &gaze_core::config::Config,
) -> Option<gaze_security::keyring::Backend> {
    use gaze_security::keyring::Backend;
    if service == Some(FACE_PAM_SERVICE) && config.storage.unlock_gnome_keyring {
        Some(Backend::Gnome)
    } else if is_kwallet_login(service) && config.storage.unlock_kwallet {
        Some(Backend::KWallet)
    } else {
        None
    }
}

fn finish_keyring<F>(
    authenticated: c_int,
    service: Option<&str>,
    config: &gaze_core::config::Config,
    supply: F,
) -> c_int
where
    F: FnOnce() -> Result<(), ()>,
{
    if authenticated != PAM_SUCCESS || keyring_backend(service, config).is_none() {
        return authenticated;
    }
    if config.storage.validate_keyring(&config.liveness).is_err() || supply().is_err() {
        return PAM_AUTHINFO_UNAVAIL;
    }
    PAM_SUCCESS
}

unsafe fn supply_keyring_token(
    pamh: PamHandle,
    username: &str,
    backend: gaze_security::keyring::Backend,
) -> Result<(), ()> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(());
    }
    let mut existing = std::ptr::null();
    if unsafe { pam_get_item(pamh, PAM_AUTHTOK, &mut existing) } != PAM_SUCCESS {
        return Err(());
    }
    // GDM's empty password placeholder must not prevent the keyring hand-off.
    if unsafe { existing_token_has_password(existing.cast()) } {
        return Ok(());
    }
    // An unenrolled user has nothing to unlock: leave the keyring locked as before rather
    // than failing the face login for everyone who has not run `gaze keyring`.
    let Some(secret) = gaze_security::keyring::load_for(backend, username).map_err(|_| ())? else {
        return Ok(());
    };
    // Linux-PAM copies the token; our zeroizing buffer is dropped immediately afterwards.
    if unsafe { pam_set_item(pamh, PAM_AUTHTOK, secret.as_ptr().cast()) } != PAM_SUCCESS {
        return Err(());
    }
    Ok(())
}

// A non-null token must point to readable PAM-owned memory.
unsafe fn existing_token_has_password(token: *const c_char) -> bool {
    !token.is_null() && unsafe { *token } != 0
}

const PROMPT_RETIRE_TIMEOUT: Duration = Duration::from_secs(2);
const PROMPT_SIGNAL_INTERVAL: Duration = Duration::from_millis(50);

async fn authenticate_biometric_with_timeout(
    username: &str,
    service: Option<&str>,
    timeout_duration: Duration,
) -> Option<AuthOutcome> {
    let auth_future = async {
        let (_config, proxy) = setup_auth_env().await.ok()?;
        authenticate_biometric_with_status_on(&proxy, username, service, false)
            .await
            .ok()
            .map(|(outcome, _)| outcome)
    };

    tokio::select! {
        res = auth_future => res,
        _ = tokio::time::sleep(timeout_duration) => None,
    }
}

fn first_pass_verdict(outcome: Option<AuthOutcome>) -> FirstPassVerdict {
    match outcome {
        Some(AuthOutcome::NoMatch) => FirstPassVerdict::NoMatch,
        _ => FirstPassVerdict::Undecided,
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

const TTY_CONVERSATION_SERVICES: [&str; 6] = ["sudo", "sudo-i", "su", "su-l", "doas", "login"];

/// Whether a prompt started now could be unblocked again, rather than parking a thread inside
/// the caller's conversation with no way back out. An open `/dev/tty` is not enough: a locker
/// launched from a shell has one, but its conversation waits on a condition variable that
/// neither an injected newline nor EINTR can end.
fn prompt_is_retirable(service: Option<&str>) -> bool {
    service_reads_the_tty(service) && has_interactive_tty()
}

fn service_reads_the_tty(service: Option<&str>) -> bool {
    service.is_some_and(|s| TTY_CONVERSATION_SERVICES.contains(&s))
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

unsafe fn do_authenticate_simultaneous(
    pamh: PamHandle,
    flags: c_int,
    _options: PamOptions,
) -> c_int {
    let service = unsafe { get_pam_service(pamh) };
    if service_defers_to_face_service(service.as_deref())
        || service_defers_to_face_slot(service.as_deref())
    {
        return PAM_IGNORE;
    }

    let silent = caller_wants_silence(flags);

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

    let (loaded_auth, pam_internal) = match rt.block_on(setup_auth_env()) {
        Ok((cfg, proxy)) => {
            let internal = rt.block_on(gaze_core::dbus::get_pam_internal(&proxy));
            (Some(cfg.auth), internal)
        }
        Err(_) => (None, Vec::new()),
    };
    let is_internal = service
        .as_deref()
        .is_some_and(|s| is_service_internal(s, &pam_internal));

    let require_confirmation = confirmation_required(loaded_auth.as_ref(), service.as_deref());
    let auth = loaded_auth.unwrap_or_default();

    let opening_prompt = if is_internal {
        GAZE_MSG_LOOK_OR_PASSWORD
    } else {
        LOOK_OR_PASSWORD_PROMPT
    };
    let prompt_line = unsafe { announce_prompt(pamh, silent, opening_prompt, is_internal) };

    let is_polkit = matches!(service, Some(ref s) if s == "polkit-1");

    // Only a terminal conversation can be retired; any other one would resume its wait after
    // every signal. Polkit is exempt: it consumes the prompt for confirmation.
    if !prompt_is_retirable(service.as_deref()) && !is_polkit {
        let bio = rt.block_on(authenticate_biometric_with_timeout(
            &username,
            service.as_deref(),
            camera_auth_timeout(&auth, service.as_deref()),
        ));
        if bio != Some(AuthOutcome::Match) {
            unsafe { record_first_pass_verdict(pamh, first_pass_verdict(bio)) };
            return PAM_AUTHINFO_UNAVAIL;
        }
        if require_confirmation {
            // Its own conversation, on this thread, so there is nothing left to unblock.
            let confirmed = if is_internal {
                unsafe { confirm_authentication_internal(pamh) }
            } else {
                unsafe { confirm_authentication(pamh, prompt_line) }
            };
            return if confirmed { PAM_SUCCESS } else { PAM_AUTH_ERR };
        }
        unsafe { report_face_verified(pamh, silent, prompt_line, is_internal) };
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
        Biometric(Option<AuthOutcome>),
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
            unsafe { record_first_pass_verdict(pamh, FirstPassVerdict::Preempted) };
            let fallback = unsafe { wait_for_password_and_fallback(pamh, &state) };
            let _ = prompt_thread.join();
            fallback
        }
        SelectorResult::Biometric(bio_res) => {
            if bio_res != Some(AuthOutcome::Match) {
                unsafe { record_first_pass_verdict(pamh, first_pass_verdict(bio_res)) };
                let fallback = unsafe { wait_for_password_and_fallback(pamh, &state) };
                let _ = prompt_thread.join();
                return fallback;
            }

            if !require_confirmation {
                retire_prompt(&state, prompt_thread);
                unsafe { report_face_verified(pamh, silent, PromptLine::Printed, is_internal) };
                return PAM_SUCCESS;
            }

            if !is_polkit {
                retire_prompt(&state, prompt_thread);
                let confirmed = if is_internal {
                    unsafe { confirm_authentication_internal(pamh) }
                } else {
                    unsafe { confirm_authentication(pamh, PromptLine::Printed) }
                };
                if confirmed { PAM_SUCCESS } else { PAM_AUTH_ERR }
            } else {
                unsafe { confirm_graphical_polkit(pamh, &state, prompt_thread, is_internal) }
            }
        }
    }
}

pub unsafe fn do_authenticate(pamh: PamHandle, flags: c_int, options: PamOptions) -> c_int {
    if caller_is_remote(unsafe { get_pam_rhost(pamh) }.as_deref()) {
        return PAM_IGNORE;
    }
    // The managed login entry owns the scan. Shared distro stacks may contain another
    // Gaze entry (including simultaneous/retry); reaching it on fallback must not scan again.
    if is_kwallet_login(unsafe { get_pam_service(pamh) }.as_deref()) {
        let mut attempted = std::ptr::null();
        if unsafe { pam_get_data(pamh, c"gaze_kde_login_attempted".as_ptr(), &mut attempted) }
            == PAM_SUCCESS
        {
            return PAM_IGNORE;
        }
        if options.kde_login
            && unsafe {
                pam_set_data(
                    pamh,
                    c"gaze_kde_login_attempted".as_ptr(),
                    c"attempted".as_ptr().cast_mut().cast(),
                    None,
                )
            } != PAM_SUCCESS
        {
            return PAM_AUTHINFO_UNAVAIL;
        }
    }
    match options.mode {
        PamMode::Sequential | PamMode::Retry => unsafe {
            do_authenticate_sequential(pamh, flags, options)
        },
        PamMode::Simultaneous => unsafe { do_authenticate_simultaneous(pamh, flags, options) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaze_core::dbus::CaptureStatus;
    use std::cell::Cell;
    use std::sync::mpsc;
    use std::sync::{Mutex, MutexGuard};
    use std::time::Instant;

    fn keyring_config() -> gaze_core::config::Config {
        let mut config = gaze_core::config::Config::default();
        config.storage.encrypt_templates = true;
        config.storage.unlock_gnome_keyring = true;
        config.liveness.enabled = true;
        config
    }

    #[test]
    fn failed_face_liveness_or_confirmation_never_reads_a_credential() {
        for failure in [
            PAM_AUTH_ERR,
            PAM_AUTHINFO_UNAVAIL,
            PAM_IGNORE,
            PAM_SERVICE_ERR,
        ] {
            assert_eq!(
                finish_keyring(
                    failure,
                    Some(FACE_PAM_SERVICE),
                    &keyring_config(),
                    || panic!("must not release")
                ),
                failure
            );
        }
    }

    #[test]
    fn keyring_is_opt_in_and_gdm_only() {
        assert_eq!(
            finish_keyring(
                PAM_SUCCESS,
                Some(FACE_PAM_SERVICE),
                &Default::default(),
                || panic!("disabled")
            ),
            PAM_SUCCESS
        );
        for service in [
            None,
            Some("sudo"),
            Some("gdm-password"),
            Some("kde-fingerprint"),
            Some("login"),
        ] {
            assert_eq!(
                finish_keyring(PAM_SUCCESS, service, &keyring_config(), || panic!(
                    "not gdm-face"
                )),
                PAM_SUCCESS
            );
        }
    }

    #[test]
    fn kwallet_release_is_limited_to_successful_enabled_kde_logins() {
        let mut config = keyring_config();
        config.storage.unlock_gnome_keyring = false;
        config.storage.unlock_kwallet = true;
        for service in ["sddm", "plasmalogin", "plasmalogin-fingerprint"] {
            for failure in [
                PAM_AUTH_ERR,
                PAM_AUTHINFO_UNAVAIL,
                PAM_IGNORE,
                PAM_SERVICE_ERR,
            ] {
                assert_eq!(
                    finish_keyring(failure, Some(service), &config, || panic!(
                        "failed authentication"
                    )),
                    failure
                );
            }
            assert_eq!(
                finish_keyring(PAM_SUCCESS, Some(service), &config, || Ok(())),
                PAM_SUCCESS
            );
            assert_eq!(
                finish_keyring(PAM_SUCCESS, Some(service), &config, || Err(())),
                PAM_AUTHINFO_UNAVAIL
            );
            config.liveness.enabled = false;
            assert_eq!(
                finish_keyring(PAM_SUCCESS, Some(service), &config, || panic!(
                    "no liveness"
                )),
                PAM_AUTHINFO_UNAVAIL
            );
            config.liveness.enabled = true;
            config.storage.encrypt_templates = false;
            assert_eq!(
                finish_keyring(PAM_SUCCESS, Some(service), &config, || panic!("no TPM")),
                PAM_AUTHINFO_UNAVAIL
            );
            config.storage.encrypt_templates = true;
        }
        for service in [
            None,
            Some("sudo"),
            Some("polkit-1"),
            Some("kde"),
            Some("kde-fingerprint"),
            Some("kde-smartcard"),
            Some("gdm-face"),
            Some("sddm-autologin"),
            Some("login"),
        ] {
            assert_eq!(
                finish_keyring(PAM_SUCCESS, service, &config, || panic!("not a KDE login")),
                PAM_SUCCESS
            );
        }
        config.storage.unlock_kwallet = false;
        assert_eq!(
            finish_keyring(PAM_SUCCESS, Some("sddm"), &config, || panic!("disabled")),
            PAM_SUCCESS
        );
        assert!(parse_pam_options(["kde-login"]).kde_login);
    }

    #[test]
    fn keyring_replaces_gdms_empty_password_placeholder() {
        assert!(!unsafe { existing_token_has_password(std::ptr::null()) });
        assert!(!unsafe { existing_token_has_password(c"".as_ptr()) });
        assert!(unsafe { existing_token_has_password(c"password".as_ptr()) });
    }

    #[test]
    fn keyring_requires_tpm_configuration_and_liveness() {
        let mut config = keyring_config();
        config.storage.encrypt_templates = false;
        assert_eq!(
            finish_keyring(PAM_SUCCESS, Some(FACE_PAM_SERVICE), &config, || panic!(
                "no TPM"
            )),
            PAM_AUTHINFO_UNAVAIL
        );
        config.storage.encrypt_templates = true;
        config.liveness.enabled = false;
        assert_eq!(
            finish_keyring(PAM_SUCCESS, Some(FACE_PAM_SERVICE), &config, || panic!(
                "no liveness"
            )),
            PAM_AUTHINFO_UNAVAIL
        );
    }

    #[test]
    fn successful_biometrics_supply_token_once_and_failure_requests_password_fallback() {
        let supplied = Cell::new(0);
        assert_eq!(
            finish_keyring(
                PAM_SUCCESS,
                Some(FACE_PAM_SERVICE),
                &keyring_config(),
                || {
                    supplied.set(supplied.get() + 1);
                    Ok(())
                }
            ),
            PAM_SUCCESS
        );
        assert_eq!(supplied.get(), 1);
        assert_eq!(
            finish_keyring(
                PAM_SUCCESS,
                Some(FACE_PAM_SERVICE),
                &keyring_config(),
                || Err(())
            ),
            PAM_AUTHINFO_UNAVAIL
        );
    }

    #[test]
    fn mode_parsing_defaults_to_sequential() {
        assert_eq!(parse_pam_mode(Vec::<&str>::new()), PamMode::Sequential);
        assert_eq!(parse_pam_mode(["debug", "silent"]), PamMode::Sequential);
    }

    #[test]
    fn mode_parsing_detects_retry() {
        assert_eq!(parse_pam_mode(["retry"]), PamMode::Retry);
        assert_eq!(parse_pam_mode(["debug", "retry"]), PamMode::Retry);
    }

    #[test]
    fn last_mode_token_wins() {
        assert_eq!(parse_pam_mode(["simultaneous", "retry"]), PamMode::Retry);
        assert_eq!(
            parse_pam_mode(["retry", "simultaneous"]),
            PamMode::Simultaneous
        );
    }

    #[test]
    fn only_terminal_conversations_are_retirable() {
        for service in TTY_CONVERSATION_SERVICES {
            assert!(service_reads_the_tty(Some(service)));
        }
        assert!(!service_reads_the_tty(Some("hyprlock")));
        assert!(!service_reads_the_tty(Some("polkit-1")));
        assert!(!service_reads_the_tty(None));
    }

    #[test]
    fn a_definitive_non_match_stands_the_retry_down() {
        assert!(!retry_is_warranted(Some(FirstPassVerdict::NoMatch)));
    }

    #[test]
    fn an_undecided_first_pass_earns_a_retry() {
        assert!(retry_is_warranted(Some(FirstPassVerdict::Undecided)));
        assert!(retry_is_warranted(Some(FirstPassVerdict::Preempted)));
    }

    #[test]
    fn a_retry_module_standing_alone_still_runs() {
        assert!(retry_is_warranted(None));
    }

    #[test]
    fn only_a_reached_non_match_hands_off_as_non_match() {
        assert_eq!(
            sequential_handoff(&Verdict::Reached(AuthOutcome::NoMatch, None)),
            Some(FirstPassVerdict::NoMatch)
        );
        assert_eq!(
            sequential_handoff(&Verdict::Reached(AuthOutcome::Unavailable, None)),
            Some(FirstPassVerdict::Undecided)
        );
        assert_eq!(
            sequential_handoff(&Verdict::Exhausted),
            Some(FirstPassVerdict::Undecided)
        );
        assert_eq!(
            sequential_handoff(&Verdict::Failed),
            Some(FirstPassVerdict::Undecided)
        );
    }

    #[test]
    fn a_match_hands_nothing_off() {
        assert_eq!(
            sequential_handoff(&Verdict::Reached(AuthOutcome::Match, None)),
            None
        );
    }

    #[test]
    fn a_timed_out_camera_is_undecided_not_a_non_match() {
        assert_eq!(first_pass_verdict(None), FirstPassVerdict::Undecided);
        assert_eq!(
            first_pass_verdict(Some(AuthOutcome::Unavailable)),
            FirstPassVerdict::Undecided
        );
        assert_eq!(
            first_pass_verdict(Some(AuthOutcome::NoMatch)),
            FirstPassVerdict::NoMatch
        );
    }

    #[test]
    fn mode_parsing_detects_simultaneous() {
        assert_eq!(parse_pam_mode(["simultaneous"]), PamMode::Simultaneous);
        assert_eq!(
            parse_pam_mode(["debug", "simultaneous", "other"]),
            PamMode::Simultaneous
        );
        assert_eq!(parse_pam_mode(["grosshack"]), PamMode::Sequential);
    }

    #[test]
    fn options_parsing_detects_modes() {
        assert_eq!(
            parse_pam_options(Vec::<&str>::new()),
            PamOptions {
                mode: PamMode::Sequential,
                kde_login: false,
            }
        );
        assert_eq!(
            parse_pam_options(["simultaneous"]),
            PamOptions {
                mode: PamMode::Simultaneous,
                kde_login: false,
            }
        );
        assert_eq!(
            parse_pam_options(["other", "simultaneous"]),
            PamOptions {
                mode: PamMode::Simultaneous,
                kde_login: false,
            }
        );
    }

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

    static PROCESS_WIDE_SIGNAL_DISPOSITION: Mutex<()> = Mutex::new(());

    fn exclusive_signals() -> MutexGuard<'static, ()> {
        PROCESS_WIDE_SIGNAL_DISPOSITION
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

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
        let _signals = exclusive_signals();
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
        let _signals = exclusive_signals();
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

        let _signals = exclusive_signals();
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
        let _signals = exclusive_signals();
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
        let _signals = exclusive_signals();
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
