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

    let config = match rt.block_on(setup_auth_env()) {
        Ok((cfg, _)) => cfg,
        Err(_) => gaze_core::config::Config::default(),
    };
    let require_confirmation = config.auth.require_confirmation;

    unsafe { say(pamh, "Please look at the camera or enter password") };

    let state: SharedAuthState = Arc::new((
        Mutex::new(AuthState {
            password: None,
            finished: false,
        }),
        Condvar::new(),
    ));

    let notify = Arc::new(tokio::sync::Notify::new());

    let thread_state = Arc::clone(&state);
    let notify_clone = Arc::clone(&notify);
    let pamh_worker = pamh as usize;
    let prompt_thread = thread::spawn(move || {
        let password = unsafe { prompt_password(pamh_worker as PamHandle) };
        let (lock, condvar) = &*thread_state;
        let mut shared_state = lock.lock();
        if let Some(pw) = password {
            shared_state.password = Some(pw);
        }
        shared_state.finished = true;
        condvar.notify_all();
        notify_clone.notify_one();
    });

    let biometric_fut = authenticate_biometric_with_timeout(&username);
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
            let fallback = wait_for_password_and_fallback(pamh, &state);
            let _ = prompt_thread.join();
            fallback
        }
        SelectorResult::Biometric(bio_res) => {
            if bio_res != Some(PAM_SUCCESS) {
                let fallback = wait_for_password_and_fallback(pamh, &state);
                let _ = prompt_thread.join();
                return fallback;
            }

            if !require_confirmation {
                if unblock_terminal() {
                    wait_for_prompt_finish(&state);
                    let _ = prompt_thread.join();
                }
                return PAM_SUCCESS;
            }

            let is_polkit =
                matches!(unsafe { get_pam_service(pamh) }, Some(ref s) if s == "polkit-1");

            if !is_polkit {
                if unblock_terminal() {
                    wait_for_prompt_finish(&state);
                }
                let _ = prompt_thread.join();

                if unsafe { confirm_authentication(pamh) } {
                    PAM_SUCCESS
                } else {
                    PAM_AUTH_ERR
                }
            } else {
                let active_uid = rt
                    .block_on(async { gaze_core::dbus::get_active_session_uid().await.ok() })
                    .or_else(|| get_user_uid(&username));

                let de = active_uid
                    .map(detect_desktop_environment)
                    .unwrap_or_else(|| "Other".to_string());

                if de == "GNOME" {
                    let is_ext_active = rt.block_on(async {
                        if let Ok((_config, proxy)) = setup_auth_env().await {
                            if let Some(uid) = active_uid {
                                proxy.is_extension_active(uid).await.unwrap_or(false)
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    });

                    if is_ext_active {
                        unsafe { say(pamh, "GAZE_CONFIRMATION_REQUEST") };

                        let (lock, condvar) = &*state;
                        let mut shared_state = lock.lock();
                        while !shared_state.finished {
                            condvar.wait(&mut shared_state);
                        }
                        let response = shared_state.password.clone();
                        drop(shared_state);
                        let _ = prompt_thread.join();

                        if let Some(resp) = response {
                            if resp == "CONFIRM" {
                                PAM_SUCCESS
                            } else {
                                stash_password_and_fallback(pamh, &resp)
                            }
                        } else {
                            PAM_AUTH_ERR
                        }
                    } else {
                        PAM_SUCCESS
                    }
                } else {
                    let prompt = match de.as_str() {
                        "KDE" | "LXQt" => "Face Verified. Press OK to confirm.",
                        "Hyprland" => "Face Verified. Press Authenticate to confirm.",
                        _ => "Face Verified. Press Enter to confirm.",
                    };

                    unsafe { say(pamh, prompt) };

                    let (lock, condvar) = &*state;
                    let mut shared_state = lock.lock();
                    while !shared_state.finished {
                        condvar.wait(&mut shared_state);
                    }
                    let response = shared_state.password.clone();
                    drop(shared_state);
                    let _ = prompt_thread.join();

                    if let Some(resp) = response {
                        if resp.is_empty() {
                            PAM_SUCCESS
                        } else {
                            stash_password_and_fallback(pamh, &resp)
                        }
                    } else {
                        PAM_AUTH_ERR
                    }
                }
            }
        }
    }
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
