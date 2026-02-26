#![allow(clippy::missing_safety_doc)]
use gaze_core::capture::frame_to_bytes;
use pam_gaze_core::*;
use parking_lot::Mutex;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Arc;
use std::thread;

struct AuthState {
    password: Option<String>,
    finished: bool,
}

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let username = match unsafe { get_username(pamh) } {
        Some(u) => u,
        None => return PAM_AUTH_ERR,
    };

    let env = setup_auth_env();
    if env.is_ok() {
        unsafe { say(pamh, "Please look at the camera or enter password") };
    }

    let state = Arc::new(Mutex::new(AuthState {
        password: None,
        finished: false,
    }));

    let thread_state = Arc::clone(&state);
    let pamh_worker = pamh as usize;
    let _ = thread::spawn(move || {
        if let Some(pw) = unsafe { prompt_password(pamh_worker as PamHandle) } {
            let mut s = thread_state.lock();
            s.password = Some(pw);
            s.finished = true;
        } else {
            thread_state.lock().finished = true;
        }
    });

    if let Ok((_, mut cam, proxy)) = env {
        for _ in 0..MAX_ATTEMPTS {
            {
                let s = state.lock();
                if s.finished {
                    if let Some(ref pw) = s.password {
                        let pw_cstr = CString::new(pw.as_str()).unwrap();
                        unsafe {
                            pam_set_item(pamh, PAM_AUTHTOK, pw_cstr.as_ptr() as *const c_void);
                        }
                        return PAM_AUTHINFO_UNAVAIL;
                    }
                    return PAM_AUTH_ERR;
                }
            }

            let frame = match cam.capture_frame() {
                Ok(f) => f,
                Err(_) => {
                    thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
            };
            let capture = match frame_to_bytes(&frame) {
                Ok(c) => c,
                Err(_) => continue,
            };

            match proxy.authenticate(&username, &capture.bytes, capture.width, capture.height) {
                Ok(face) if !face.is_empty() => {
                    drop(cam);
                    unblock_terminal();
                    return PAM_SUCCESS;
                }
                Ok(_) => {}
                Err(ref err) if is_retryable(err) => continue,
                Err(_) => break,
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
        drop(cam);
    }

    loop {
        {
            let s = state.lock();
            if s.finished {
                if let Some(ref pw) = s.password {
                    let pw_cstr = CString::new(pw.as_str()).unwrap();
                    unsafe {
                        pam_set_item(pamh, PAM_AUTHTOK, pw_cstr.as_ptr() as *const c_void);
                    }
                    return PAM_AUTHINFO_UNAVAIL;
                }
                return PAM_AUTH_ERR;
            }
        }
        thread::sleep(std::time::Duration::from_millis(50));
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
