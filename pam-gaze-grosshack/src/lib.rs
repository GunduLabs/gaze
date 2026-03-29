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
    let _ = thread::spawn(move || {
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

    rt.block_on(async {
        let auth_future = authenticate_biometric(&username);
        let timeout_duration = Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS);

        let result = tokio::select! {
            res = auth_future => {
                match res {
                    Ok(true) => {
                        unblock_terminal();
                        Some(PAM_SUCCESS)
                    }
                    Ok(false) => Some(PAM_AUTH_ERR),
                    Err(_) => None,
                }
            }
            _ = tokio::time::sleep(timeout_duration) => None,
        };

        if result == Some(PAM_SUCCESS) {
            return PAM_SUCCESS;
        }

        wait_for_password_and_fallback(pamh, &state)
    })
}

fn unblock_terminal() {
    unsafe {
        let nl = b'\n' as libc::c_char;
        libc::ioctl(0, libc::TIOCSTI, &nl as *const libc::c_char);
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
