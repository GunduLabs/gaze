#![allow(clippy::missing_safety_doc)]
use parking_lot::{Condvar, Mutex};
use std::ffi::{CStr, CString};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::AsRawFd;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::thread;

use gaze_core::config::Config;

pub const PAM_SUCCESS: c_int = 0;
pub const PAM_AUTH_ERR: c_int = 7;
pub const PAM_SERVICE_ERR: c_int = 3;
pub const PAM_CONV: c_int = 5;
pub const PAM_SERVICE: c_int = 1;
pub const PAM_AUTHTOK: c_int = 6;
pub const PAM_TEXT_INFO: c_int = 4;
pub const PAM_PROMPT_ECHO_OFF: c_int = 1;
pub const PAM_PROMPT_ECHO_ON: c_int = 2;
pub const PAM_AUTHINFO_UNAVAIL: c_int = 9;
pub const PAM_IGNORE: c_int = 25;

pub const CAMERA_AUTH_TIMEOUT_SECS: u64 = 12;
const CONFIRMATION_PROMPT: &str = "Face Verified. Press Enter to confirm, Esc to cancel.";
pub const CONFIRMATION_REQUEST: &str = "GAZE_CONFIRMATION_REQUEST";
pub const CONFIRMATION_ACK: &str = "CONFIRM";

pub type PamHandle = *mut c_void;

#[macro_export]
macro_rules! pam_success_stubs {
    () => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn pam_sm_setcred(
            _pamh: $crate::PamHandle,
            _flags: ::std::os::raw::c_int,
            _argc: ::std::os::raw::c_int,
            _argv: *const *const ::std::os::raw::c_char,
        ) -> ::std::os::raw::c_int {
            $crate::PAM_SUCCESS
        }

        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn pam_sm_acct_mgmt(
            _pamh: $crate::PamHandle,
            _flags: ::std::os::raw::c_int,
            _argc: ::std::os::raw::c_int,
            _argv: *const *const ::std::os::raw::c_char,
        ) -> ::std::os::raw::c_int {
            $crate::PAM_SUCCESS
        }
    };
}

#[repr(C)]
pub struct PamMessage {
    pub msg_style: c_int,
    pub msg: *const c_char,
}

#[repr(C)]
pub struct PamResponse {
    pub resp: *mut c_char,
    pub resp_retcode: c_int,
}

#[repr(C)]
pub struct PamConv {
    pub conv: Option<
        unsafe extern "C" fn(
            num_msg: c_int,
            msg: *mut *const PamMessage,
            resp: *mut *mut PamResponse,
            appdata_ptr: *mut c_void,
        ) -> c_int,
    >,
    pub appdata_ptr: *mut c_void,
}

unsafe extern "C" {
    pub fn pam_get_user(pamh: PamHandle, user: *mut *const c_char, prompt: *const c_char) -> c_int;
    pub fn pam_get_item(pamh: PamHandle, item_type: c_int, item: *mut *const c_void) -> c_int;
    pub fn pam_set_item(pamh: PamHandle, item_type: c_int, item: *const c_void) -> c_int;
}

pub unsafe fn converse(pamh: PamHandle, msg_style: c_int, text: &str) -> Option<String> {
    unsafe {
        let mut item: *const c_void = ptr::null();
        if pam_get_item(pamh, PAM_CONV, &mut item) != PAM_SUCCESS || item.is_null() {
            return None;
        }
        let conv = &*(item as *const PamConv);
        let conv_fn = conv.conv?;

        let Ok(msg_str) = CString::new(text) else {
            return None;
        };
        let msg = PamMessage {
            msg_style,
            msg: msg_str.as_ptr(),
        };
        let mut msg_ptr = &msg as *const PamMessage;
        let mut resp_ptr: *mut PamResponse = ptr::null_mut();

        if (conv_fn)(1, &mut msg_ptr, &mut resp_ptr, conv.appdata_ptr) != PAM_SUCCESS {
            return None;
        }

        let mut result = None;
        if !resp_ptr.is_null() {
            let resp = (*resp_ptr).resp;
            if !resp.is_null() {
                result = Some(CStr::from_ptr(resp).to_string_lossy().into_owned());
                libc::free(resp as *mut c_void);
            }
            libc::free(resp_ptr as *mut c_void);
        }
        result
    }
}

struct TermiosGuard {
    fd: c_int,
    original: libc::termios,
}

impl Drop for TermiosGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
        }
    }
}

fn confirm_from_tty() -> Option<bool> {
    let mut tty = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()?;
    let fd = tty.as_raw_fd();

    let mut original = MaybeUninit::<libc::termios>::uninit();
    unsafe {
        if libc::tcgetattr(fd, original.as_mut_ptr()) != 0 {
            return None;
        }
        let original = original.assume_init();
        let mut raw = original;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return None;
        }

        let _guard = TermiosGuard { fd, original };
        write!(tty, "\x1B[1A\x1B[2K\r{CONFIRMATION_PROMPT}").ok()?;
        tty.flush().ok()?;

        let mut key = [0_u8; 1];
        tty.read_exact(&mut key).ok()?;
        writeln!(tty).ok()?;

        let confirmed = matches!(key[0], b'\n' | b'\r');
        Some(confirmed)
    }
}
pub fn has_controlling_tty() -> bool {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
}

pub unsafe fn confirm_authentication(pamh: PamHandle) -> bool {
    if let Some(confirmed) = confirm_from_tty() {
        return confirmed;
    }

    unsafe { converse(pamh, PAM_PROMPT_ECHO_ON, CONFIRMATION_PROMPT) }
        .map(|resp| resp.is_empty())
        .unwrap_or(false)
}

pub fn confirmation_accepted(response: Option<&str>) -> bool {
    matches!(response, Some(CONFIRMATION_ACK))
}

pub struct AuthState {
    pub password: Option<String>,
    pub started: bool,
    pub finished: bool,
}

pub type SharedAuthState = Arc<(Mutex<AuthState>, Condvar)>;

pub fn new_auth_state() -> SharedAuthState {
    Arc::new((
        Mutex::new(AuthState {
            password: None,
            started: false,
            finished: false,
        }),
        Condvar::new(),
    ))
}

pub fn spawn_prompt_thread(
    pamh: PamHandle,
    state: &SharedAuthState,
    on_finished: impl FnOnce() + Send + 'static,
) -> thread::JoinHandle<()> {
    let thread_state = Arc::clone(state);
    let pamh_worker = pamh as usize;
    thread::spawn(move || {
        {
            let (lock, condvar) = &*thread_state;
            let mut shared_state = lock.lock();
            shared_state.started = true;
            condvar.notify_all();
        }
        let password = unsafe { prompt_password(pamh_worker as PamHandle) };
        {
            let (lock, condvar) = &*thread_state;
            let mut shared_state = lock.lock();
            if let Some(pw) = password {
                shared_state.password = Some(pw);
            }
            shared_state.finished = true;
            condvar.notify_all();
        }
        on_finished();
    })
}

pub fn wait_for_prompt_started(state: &SharedAuthState) {
    let (lock, condvar) = &**state;
    let mut shared_state = lock.lock();
    while !shared_state.started {
        condvar.wait(&mut shared_state);
    }
}

pub fn wait_for_prompt_finish(state: &SharedAuthState) {
    let (lock, condvar) = &**state;
    let mut shared_state = lock.lock();
    while !shared_state.finished {
        condvar.wait(&mut shared_state);
    }
}

pub fn wait_for_prompt_response(state: &SharedAuthState) -> Option<String> {
    let (lock, condvar) = &**state;
    let mut shared_state = lock.lock();
    while !shared_state.finished {
        condvar.wait(&mut shared_state);
    }
    shared_state.password.clone()
}

pub unsafe fn wait_for_password_and_fallback(pamh: PamHandle, state: &SharedAuthState) -> c_int {
    let (lock, condvar) = &**state;
    let mut shared_state = lock.lock();
    loop {
        if shared_state.finished {
            if let Some(ref pw) = shared_state.password {
                return unsafe { stash_password_and_fallback(pamh, pw) };
            }
            return PAM_AUTH_ERR;
        }
        condvar.wait(&mut shared_state);
    }
}

pub unsafe fn stash_password_and_fallback(pamh: PamHandle, password: &str) -> c_int {
    // Password contained a NUL byte, so fail rather than panic.
    let Ok(pw_cstr) = CString::new(password) else {
        return PAM_AUTH_ERR;
    };
    unsafe {
        pam_set_item(pamh, PAM_AUTHTOK, pw_cstr.as_ptr() as *const c_void);
    }
    PAM_AUTHINFO_UNAVAIL
}

pub fn polkit_confirm_message(de: &str) -> &'static str {
    match de {
        "GNOME" => CONFIRMATION_REQUEST,
        "KDE" | "LXQt" => "Face Verified. Press OK to confirm.",
        "Hyprland" => "Face Verified. Press Authenticate to confirm.",
        _ => "Face Verified. Press Enter to confirm.",
    }
}

// Confirm a face match through a graphical polkit dialog; the caller must
// already have a pending password prompt on `state` for the agent to answer.
pub unsafe fn confirm_graphical_polkit(
    pamh: PamHandle,
    de: &str,
    extension_active: bool,
    state: &SharedAuthState,
    prompt_thread: thread::JoinHandle<()>,
) -> c_int {
    if de == "GNOME" && !extension_active {
        let fallback = unsafe { wait_for_password_and_fallback(pamh, state) };
        let _ = prompt_thread.join();
        return fallback;
    }

    unsafe { say(pamh, polkit_confirm_message(de)) };

    let response = wait_for_prompt_response(state);
    let _ = prompt_thread.join();

    let Some(resp) = response else {
        return PAM_AUTH_ERR;
    };
    let confirmed = if de == "GNOME" {
        resp == CONFIRMATION_ACK
    } else {
        resp.is_empty()
    };
    if confirmed {
        PAM_SUCCESS
    } else {
        unsafe { stash_password_and_fallback(pamh, &resp) }
    }
}

pub async fn active_or_user_uid(username: &str) -> Option<u32> {
    match gaze_core::dbus::get_active_session_uid().await {
        Ok(uid) => Some(uid),
        Err(_) => get_user_uid(username),
    }
}

// Like `active_or_user_uid`, but also reports whether the active seat session
// is a login greeter (e.g. GDM). A greeter always runs GNOME with the gaze
// extension loaded, yet its short-lived processes make `/proc`-based DE
// detection unreliable, so callers gate confirmation on this flag instead.
pub async fn active_confirm_target(username: &str) -> (Option<u32>, bool) {
    match gaze_core::dbus::get_active_session_uid_and_class().await {
        Ok((uid, class)) => (Some(uid), class == "greeter"),
        Err(_) => (get_user_uid(username), false),
    }
}

pub async fn gnome_extension_active(uid: Option<u32>) -> bool {
    let Some(uid) = uid else {
        return false;
    };
    match setup_auth_env().await {
        Ok((_config, proxy)) => proxy.is_extension_active(uid).await.unwrap_or(false),
        Err(_) => false,
    }
}

pub unsafe fn say(pamh: PamHandle, text: &str) {
    unsafe {
        let _ = converse(pamh, PAM_TEXT_INFO, text);
    }
}

pub unsafe fn prompt_password(pamh: PamHandle) -> Option<String> {
    unsafe { converse(pamh, PAM_PROMPT_ECHO_OFF, "Password: ") }
}

pub unsafe fn get_username(pamh: PamHandle) -> Option<String> {
    let mut user_ptr: *const c_char = ptr::null();
    let ret = unsafe { pam_get_user(pamh, &mut user_ptr, ptr::null()) };
    if ret != PAM_SUCCESS || user_ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(user_ptr).to_str().ok().map(|s| s.to_owned()) }
}

pub unsafe fn username_and_runtime(
    pamh: PamHandle,
) -> Result<(String, tokio::runtime::Runtime), c_int> {
    let Some(username) = (unsafe { get_username(pamh) }) else {
        return Err(PAM_AUTH_ERR);
    };

    let rt = tokio::runtime::Runtime::new().map_err(|_| PAM_AUTHINFO_UNAVAIL)?;
    Ok((username, rt))
}

pub fn is_retryable(err: &zbus::Error) -> bool {
    err.to_string().contains("RETRYABLE:")
}

use gaze_core::dbus::GazeProxy;

pub async fn setup_auth_env() -> Result<(Config, GazeProxy<'static>), c_int> {
    let proxy = gaze_core::dbus::connect_gaze()
        .await
        .map_err(|_| PAM_SERVICE_ERR)?;
    let config = gaze_core::dbus::load_config_from_daemon(&proxy)
        .await
        .map_err(|_| PAM_SERVICE_ERR)?;
    Ok((config, proxy))
}

pub async fn has_enrolled_faces(username: &str) -> anyhow::Result<bool> {
    let (_config, proxy) = setup_auth_env()
        .await
        .map_err(|e| anyhow::anyhow!("PAM error: {}", e))?;
    match proxy.list_faces(username).await {
        // Treat unenrolled users as having no faces.
        Ok(faces) => Ok(!faces.is_empty()),
        Err(ref err) if gaze_core::dbus::dbus_is_file_not_found(err) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentDisposition {
    Continue,
    Ignore,
    Unavailable,
}

pub fn enrollment_disposition<E>(result: Result<bool, E>) -> EnrollmentDisposition {
    match result {
        Ok(true) => EnrollmentDisposition::Continue,
        Ok(false) => EnrollmentDisposition::Ignore,
        Err(_) => EnrollmentDisposition::Unavailable,
    }
}

struct ReleaseGuard {
    proxy: GazeProxy<'static>,
    active: bool,
}

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        if self.active {
            let proxy = self.proxy.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = proxy.release().await;
                });
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome {
    Match,
    NoMatch,
    Unavailable,
}

fn auth_outcome(
    result: gaze_core::dbus::VerifyResult,
    last_status: Option<gaze_core::dbus::CaptureStatus>,
) -> AuthOutcome {
    match result {
        gaze_core::dbus::VerifyResult::VerifyMatch => AuthOutcome::Match,
        gaze_core::dbus::VerifyResult::VerifyNoMatch => match last_status {
            Some(
                gaze_core::dbus::CaptureStatus::TooDark | gaze_core::dbus::CaptureStatus::NoFace,
            ) => AuthOutcome::Unavailable,
            _ => AuthOutcome::NoMatch,
        },
    }
}

pub async fn authenticate_biometric(username: &str) -> anyhow::Result<AuthOutcome> {
    let (_config, proxy) = setup_auth_env()
        .await
        .map_err(|e| anyhow::anyhow!("PAM error: {}", e))?;

    proxy
        .claim(username)
        .await
        .map_err(|e| anyhow::anyhow!("Claim failed: {:?}", e))?;

    let mut guard = ReleaseGuard {
        proxy: proxy.clone(),
        active: true,
    };

    let mut verify_stream = proxy
        .receive_verify_status()
        .await
        .map_err(|e| anyhow::anyhow!("Stream failed: {}", e))?;
    let mut face_stream = proxy
        .receive_face_status()
        .await
        .map_err(|e| anyhow::anyhow!("Stream failed: {}", e))?;
    proxy
        .verify_start("any")
        .await
        .map_err(|e| anyhow::anyhow!("Verify start failed: {}", e))?;

    use futures::StreamExt;
    let mut last_status: Option<gaze_core::dbus::CaptureStatus> = None;
    let outcome = loop {
        tokio::select! {
            Some(signal) = verify_stream.next() => {
                if let Ok(args) = signal.args() {
                    break auth_outcome(*args.result(), last_status);
                }
            }
            Some(signal) = face_stream.next() => {
                if let Ok(args) = signal.args() {
                    last_status = Some(*args.status());
                }
            }
            // Both streams ended (bus connection lost): without this branch
            // select! panics, which would abort the PAM host process.
            else => break AuthOutcome::Unavailable,
        }
    };

    guard.active = false;
    let _ = proxy.release().await;
    Ok(outcome)
}

pub fn get_user_uid(username: &str) -> Option<u32> {
    let username_cstr = CString::new(username).ok()?;
    unsafe {
        let pwd = libc::getpwnam(username_cstr.as_ptr());
        if !pwd.is_null() {
            Some((*pwd).pw_uid)
        } else {
            None
        }
    }
}

pub unsafe fn get_pam_service(pamh: PamHandle) -> Option<String> {
    let mut service_ptr: *const c_void = std::ptr::null();
    let ret = unsafe { pam_get_item(pamh, PAM_SERVICE, &mut service_ptr) };
    if ret != PAM_SUCCESS || service_ptr.is_null() {
        return None;
    }
    unsafe {
        CStr::from_ptr(service_ptr as *const c_char)
            .to_str()
            .ok()
            .map(|s| s.to_owned())
    }
}

pub fn detect_desktop_environment(uid: u32) -> String {
    let mut is_kde = false;
    let mut is_hyprland = false;
    let mut is_gnome = false;

    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata()
                && metadata.is_dir()
            {
                let path = entry.path();
                if let Some(pid_str) = path.file_name().and_then(|s| s.to_str())
                    && pid_str.chars().all(|c| c.is_ascii_digit())
                {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.uid() == uid
                        && let Ok(comm) = std::fs::read_to_string(path.join("comm"))
                    {
                        let comm_trim = comm.trim();
                        if comm_trim == "plasmashell"
                            || comm_trim == "kwin_wayland"
                            || comm_trim == "kwin_x11"
                            || comm_trim == "lxqt-policykit-agent"
                            || comm_trim == "lxqt-policykit"
                        {
                            is_kde = true;
                        } else if comm_trim == "hyprland" || comm_trim == "Hyprland" {
                            is_hyprland = true;
                        } else if comm_trim == "gnome-shell" {
                            is_gnome = true;
                        }
                    }
                }
            }
        }
    }

    if is_kde {
        "KDE".to_string()
    } else if is_hyprland {
        "Hyprland".to_string()
    } else if is_gnome {
        "GNOME".to_string()
    } else {
        "Other".to_string()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum GraphicalConfirm {
    GnomeExtension,
    FailClosed,
}

pub fn graphical_confirm_decision(
    de: &str,
    extension_active: bool,
    is_greeter: bool,
) -> GraphicalConfirm {
    if is_greeter {
        return if extension_active {
            GraphicalConfirm::GnomeExtension
        } else {
            GraphicalConfirm::FailClosed
        };
    }
    match de {
        "GNOME" if extension_active => GraphicalConfirm::GnomeExtension,
        _ => GraphicalConfirm::FailClosed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_with_active_extension_confirms_through_it() {
        assert_eq!(
            graphical_confirm_decision("GNOME", true, false),
            GraphicalConfirm::GnomeExtension
        );
    }

    #[test]
    fn gnome_without_extension_fails_closed() {
        assert_eq!(
            graphical_confirm_decision("GNOME", false, false),
            GraphicalConfirm::FailClosed
        );
    }

    #[test]
    fn other_desktops_fail_closed_without_a_channel() {
        for de in ["KDE", "Hyprland", "LXQt", "Other"] {
            assert_eq!(
                graphical_confirm_decision(de, false, false),
                GraphicalConfirm::FailClosed,
                "{de} has no confirm channel and must fail closed, not bypass"
            );
        }
    }

    #[test]
    fn greeter_confirms_through_extension_when_active() {
        // A greeter's DE detection is unreliable, so the flag decides -- even
        // if `/proc` scanning reported a non-GNOME desktop.
        assert_eq!(
            graphical_confirm_decision("Other", true, true),
            GraphicalConfirm::GnomeExtension
        );
    }

    #[test]
    fn greeter_never_bypasses_and_fails_closed_without_extension() {
        for de in ["GNOME", "KDE", "Hyprland", "Other"] {
            assert_eq!(
                graphical_confirm_decision(de, false, true),
                GraphicalConfirm::FailClosed,
                "a greeter ({de}) must fail closed, never bypass, without a confirm channel"
            );
        }
    }

    #[test]
    fn retryable_errors_are_detected_from_error_text() {
        let err = zbus::Error::Failure("RETRYABLE: camera is busy".to_string());
        assert!(is_retryable(&err));
    }

    #[test]
    fn ordinary_errors_are_not_retryable() {
        let err = zbus::Error::Failure("camera is unavailable".to_string());
        assert!(!is_retryable(&err));
    }

    #[test]
    fn enrollment_gate_ignores_unenrolled_users_but_fails_closed_on_daemon_errors() {
        assert_eq!(
            enrollment_disposition::<()>(Ok(true)),
            EnrollmentDisposition::Continue
        );
        assert_eq!(
            enrollment_disposition::<()>(Ok(false)),
            EnrollmentDisposition::Ignore
        );
        assert_eq!(
            enrollment_disposition::<&str>(Err("daemon unavailable")),
            EnrollmentDisposition::Unavailable
        );
    }

    #[test]
    fn confirmation_accepts_only_the_ack_token() {
        assert!(confirmation_accepted(Some("CONFIRM")));
        assert!(!confirmation_accepted(Some("")));
        assert!(!confirmation_accepted(Some("hunter2")));
        assert!(!confirmation_accepted(Some("confirm")));
        assert!(!confirmation_accepted(None));
    }

    #[test]
    fn polkit_confirm_message_uses_extension_token_only_on_gnome() {
        assert_eq!(polkit_confirm_message("GNOME"), CONFIRMATION_REQUEST);
        assert_eq!(
            polkit_confirm_message("KDE"),
            "Face Verified. Press OK to confirm."
        );
        assert_eq!(
            polkit_confirm_message("Hyprland"),
            "Face Verified. Press Authenticate to confirm."
        );
        assert_eq!(
            polkit_confirm_message("Other"),
            "Face Verified. Press Enter to confirm."
        );
    }

    #[test]
    fn too_dark_no_match_is_reported_as_unavailable() {
        use gaze_core::dbus::{CaptureStatus, VerifyResult};

        assert_eq!(
            auth_outcome(VerifyResult::VerifyNoMatch, Some(CaptureStatus::TooDark)),
            AuthOutcome::Unavailable
        );
        assert_eq!(
            auth_outcome(VerifyResult::VerifyNoMatch, Some(CaptureStatus::NoFace)),
            AuthOutcome::Unavailable
        );
        assert_eq!(
            auth_outcome(VerifyResult::VerifyNoMatch, Some(CaptureStatus::Usable)),
            AuthOutcome::NoMatch
        );
        assert_eq!(
            auth_outcome(VerifyResult::VerifyMatch, Some(CaptureStatus::TooDark)),
            AuthOutcome::Match
        );
    }
}
