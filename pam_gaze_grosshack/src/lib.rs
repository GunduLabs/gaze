#![allow(clippy::missing_safety_doc)]
use gaze_core::capture::{init_camera_and_checker, wait_for_capture_until};
use pam_gaze_core::*;
use parking_lot::Mutex;
use std::ffi::CString;
use std::os::raw::c_void;
use std::os::raw::{c_char, c_int};
use std::sync::Arc;
use std::thread;

struct AuthState {
    password: Option<String>,
    finished: bool,
}

fn wait_for_password_and_fallback(pamh: PamHandle, state: &Arc<Mutex<AuthState>>) -> c_int {
    loop {
        let shared_state = state.lock();
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
        drop(shared_state);
        thread::sleep(std::time::Duration::from_millis(50));
    }
}

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let username = match unsafe { get_username(pamh) } {
        Some(u) => u,
        None => return PAM_AUTH_ERR,
    };

    let Ok((config, _, proxy)) = setup_auth_env() else {
        return PAM_AUTHINFO_UNAVAIL;
    };
    let is_polkit = unsafe { is_polkit_service(pamh) };

    unsafe { say(pamh, "Please look at the camera or enter password") };

    let state = Arc::new(Mutex::new(AuthState {
        password: None,
        finished: false,
    }));

    let thread_state = Arc::clone(&state);
    let pamh_worker = pamh as usize;
    let _ = thread::spawn(move || {
        if let Some(pw) = unsafe { prompt_password(pamh_worker as PamHandle) } {
            let mut shared_state = thread_state.lock();
            shared_state.password = Some(pw);
            shared_state.finished = true;
        } else {
            thread_state.lock().finished = true;
        }
    });

    let Ok((mut cam, mut checker)) = init_camera_and_checker(&config.cameras.rgb) else {
        return PAM_SERVICE_ERR;
    };

    let capture = match wait_for_capture_until(
        &mut cam,
        &mut checker,
        false,
        |status| {
            if is_polkit {
                unsafe { say(pamh, &status.to_string()) };
            }
        },
        || state.lock().finished,
    ) {
        Ok(Some(capture)) => capture,
        Ok(None) => return wait_for_password_and_fallback(pamh, &state),
        Err(_) => return PAM_SERVICE_ERR,
    };

    match proxy.verify(&username, &capture.bytes, capture.width, capture.height) {
        Ok(true) => {
            drop(cam);
            unblock_terminal();
            PAM_SUCCESS
        }
        Ok(false) => wait_for_password_and_fallback(pamh, &state),
        Err(_) => wait_for_password_and_fallback(pamh, &state),
    }
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
