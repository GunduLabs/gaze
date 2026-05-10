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
                let pw_cstr = CString::new(pw.as_str()).unwrap();
                unsafe {
                    pam_set_item(pamh, PAM_AUTHTOK, pw_cstr.as_ptr() as *const c_void);
                }
                return PAM_AUTHINFO_UNAVAIL;
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

    unsafe { say(pamh, "Please look at the camera or enter password") };

    let state: SharedAuthState = Arc::new((
        Mutex::new(AuthState {
            password: None,
            finished: false,
        }),
        Condvar::new(),
    ));

    let thread_state = Arc::clone(&state);
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

    let biometric_result = rt.block_on(async {
        let auth_future = authenticate_biometric(&username);
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
    });

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
