#![allow(clippy::missing_safety_doc)]
use pam_gaze_core::*;
use std::os::fd::AsRawFd;
use std::os::raw::{c_char, c_int};
use std::os::unix::thread::JoinHandleExt;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

async fn authenticate_biometric_with_timeout(username: &str) -> Option<c_int> {
    let auth_future = authenticate_biometric(username);
    let timeout_duration = Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS);

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

unsafe fn install_interrupt_handler() -> Option<libc::sigaction> {
    unsafe {
        let mut new_action: libc::sigaction = std::mem::zeroed();
        new_action.sa_sigaction = interrupt_noop_handler as *const () as usize;
        libc::sigemptyset(&mut new_action.sa_mask);
        new_action.sa_flags = 0;
        let mut old_action: libc::sigaction = std::mem::zeroed();
        if libc::sigaction(libc::SIGUSR1, &new_action, &mut old_action) == 0 {
            Some(old_action)
        } else {
            None
        }
    }
}

unsafe fn restore_interrupt_handler(old: Option<libc::sigaction>) {
    if let Some(old) = old {
        unsafe {
            libc::sigaction(libc::SIGUSR1, &old, std::ptr::null_mut());
        }
    }
}

fn retire_prompt(state: &SharedAuthState, prompt_thread: thread::JoinHandle<()>) {
    if unblock_terminal() {
        wait_for_prompt_finish(state);
        let _ = prompt_thread.join();
        return;
    }

    let tid = prompt_thread.as_pthread_t();
    let old_handler = unsafe { install_interrupt_handler() };

    {
        let (lock, condvar) = &**state;
        let mut shared_state = lock.lock();
        while !shared_state.finished {
            unsafe {
                libc::pthread_kill(tid, libc::SIGUSR1);
            }
            condvar.wait_for(&mut shared_state, Duration::from_millis(50));
        }
    }

    let _ = prompt_thread.join();
    unsafe { restore_interrupt_handler(old_handler) };
}

unsafe fn do_authenticate(pamh: PamHandle) -> c_int {
    let (username, rt) = match unsafe { username_and_runtime(pamh) } {
        Ok(ctx) => ctx,
        Err(code) => return code,
    };

    match enrollment_disposition(rt.block_on(has_enrolled_faces(&username))) {
        EnrollmentDisposition::Ignore => return PAM_IGNORE,
        EnrollmentDisposition::Unavailable => return PAM_AUTHINFO_UNAVAIL,
        EnrollmentDisposition::Continue => {}
    }

    let config = match rt.block_on(setup_auth_env()) {
        Ok((cfg, _)) => cfg,
        Err(_) => gaze_core::config::Config::default(),
    };
    let require_confirmation = config.auth.require_confirmation;

    unsafe { say(pamh, "Please look at the camera or enter password") };

    let is_polkit = matches!(unsafe { get_pam_service(pamh) }, Some(ref s) if s == "polkit-1");

    let state = new_auth_state();

    let notify = Arc::new(tokio::sync::Notify::new());
    let notify_clone = Arc::clone(&notify);
    let prompt_thread = spawn_prompt_thread(pamh, &state, move || {
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
// Inject newline via TIOCSTI to unblock the PAM conversation read thread.

fn unblock_terminal() -> bool {
    if let Ok(tty) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
    {
        let fd = tty.as_raw_fd();
        let nl = b'\n' as libc::c_char;
        unsafe { libc::ioctl(fd, libc::TIOCSTI, &nl as *const libc::c_char) == 0 }
    } else {
        false
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
