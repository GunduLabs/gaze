#![allow(clippy::missing_safety_doc)]
use pam_gaze_core::*;
use parking_lot::{Condvar, Mutex};
use std::ffi::CString;
use std::os::raw::c_void;
use std::os::raw::{c_char, c_int};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

struct AuthState {
    password: Option<String>,
    finished: bool,
}

type SharedAuthState = Arc<(Mutex<AuthState>, Condvar)>;

async fn authenticate_biometric_with_timeout(username: &str) -> Option<c_int> {
    let auth_future = authenticate_biometric(username);
    let timeout_duration = Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS);

    tokio::select! {
        res = auth_future => {
            match res {
                Ok(Some(true)) => Some(PAM_SUCCESS),
                Ok(Some(false)) => Some(PAM_AUTH_ERR),
                Ok(None) => Some(PAM_IGNORE),
                Err(_) => None,
            }
        }
        _ = tokio::time::sleep(timeout_duration) => None,
    }
}

fn stash_password_and_fallback(pamh: PamHandle, password: &str) -> c_int {
    // Stash the typed password as PAM_AUTHTOK and return AUTHINFO_UNAVAIL so the
    // stack falls through to pam_unix (or whatever follows) which will pick it up
    // instead of re-prompting the user.
    let pw_cstr = CString::new(password).unwrap();
    unsafe {
        pam_set_item(pamh, PAM_AUTHTOK, pw_cstr.as_ptr() as *const c_void);
    }
    PAM_AUTHINFO_UNAVAIL
}

unsafe fn prompt_password_and_fallback(pamh: PamHandle) -> c_int {
    match unsafe { prompt_password(pamh) } {
        Some(password) => stash_password_and_fallback(pamh, &password),
        None => PAM_AUTH_ERR,
    }
}

fn wait_for_prompt_finish(state: &SharedAuthState) {
    let (lock, condvar) = &**state;
    let mut shared_state = lock.lock();
    while !shared_state.finished {
        condvar.wait(&mut shared_state);
    }
}

fn wait_for_password_and_fallback(pamh: PamHandle, state: &SharedAuthState) -> c_int {
    let (lock, condvar) = &**state;
    let mut shared_state = lock.lock();
    loop {
        if shared_state.finished {
            if let Some(ref pw) = shared_state.password {
                return stash_password_and_fallback(pamh, pw);
            }
            return PAM_AUTH_ERR;
        }
        condvar.wait(&mut shared_state);
    }
}

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let username = match unsafe { get_username(pamh) } {
        Some(u) => u,
        None => return PAM_AUTH_ERR,
    };

    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(_) => return PAM_AUTHINFO_UNAVAIL,
    };

    if let Ok(false) = rt.block_on(has_enrolled_faces(&username)) {
        return PAM_IGNORE;
    }

    let require_confirmation = gaze_core::config::Config::load()
        .map(|c| c.auth.require_confirmation)
        .unwrap_or(false);

    if require_confirmation {
        unsafe { say(pamh, "Please look at the camera") };

        if rt.block_on(authenticate_biometric_with_timeout(&username)) == Some(PAM_SUCCESS) {
            return if unsafe { confirm_authentication(pamh) } {
                PAM_SUCCESS
            } else {
                PAM_AUTH_ERR
            };
        }

        return unsafe { prompt_password_and_fallback(pamh) };
    }

    unsafe { say(pamh, "Please look at the camera or enter password") };

    let state: SharedAuthState = Arc::new((
        Mutex::new(AuthState {
            password: None,
            finished: false,
        }),
        Condvar::new(),
    ));

    let thread_state = Arc::clone(&state);
    // Raw pointers aren't Send, so smuggle the handle across the thread boundary as a usize.
    // PAM owns the handle for the whole pam_sm_authenticate call, so it stays valid.
    let pamh_worker = pamh as usize;
    let prompt_thread = thread::spawn(move || {
        let password = unsafe { prompt_password(pamh_worker as PamHandle) };
        let (lock, condvar) = &*thread_state;
        let mut shared_state = lock.lock();
        if let Some(pw) = password {
            shared_state.password = Some(pw);
            shared_state.finished = true;
        } else {
            shared_state.finished = true;
        }
        condvar.notify_all();
    });

    let biometric_result = rt.block_on(authenticate_biometric_with_timeout(&username));

    if biometric_result == Some(PAM_SUCCESS) {
        if unblock_terminal() {
            wait_for_prompt_finish(&state);
            let _ = prompt_thread.join();
        }
        return PAM_SUCCESS;
    }

    let fallback = wait_for_password_and_fallback(pamh, &state);
    let _ = prompt_thread.join();
    fallback
}

// When biometric auth wins the race, the prompt thread is still blocked inside the PAM
// conversation read. TIOCSTI injects a newline into the controlling tty's input queue so the
// read returns and the thread can join cleanly. Returns false if stdin isn't a tty (e.g. GDM,
// SSH), in which case the caller cannot safely wait for the prompt thread to finish.
fn unblock_terminal() -> bool {
    unsafe {
        if libc::isatty(0) != 1 {
            return false;
        }

        let nl = b'\n' as libc::c_char;
        libc::ioctl(0, libc::TIOCSTI, &nl as *const libc::c_char) == 0
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
