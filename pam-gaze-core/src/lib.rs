// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

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
pub const PAM_ERROR_MSG: c_int = 3;
pub const PAM_PROMPT_ECHO_OFF: c_int = 1;
pub const PAM_PROMPT_ECHO_ON: c_int = 2;
pub const PAM_AUTHINFO_UNAVAIL: c_int = 9;
pub const PAM_IGNORE: c_int = 25;

pub const PAM_DISALLOW_NULL_AUTHTOK: c_int = 0x0001;
pub const PAM_SILENT: c_int = 0x8000;

pub fn caller_wants_silence(flags: c_int) -> bool {
    flags & PAM_SILENT != 0
}

pub const CAMERA_AUTH_TIMEOUT_SECS: u64 = 12;
pub const TTY_CONFIRM_DECISECONDS: libc::cc_t = 200;
const _: () = assert!(TTY_CONFIRM_DECISECONDS > 0);
pub const FACE_PAM_SERVICE: &str = "gdm-face";

/// Camera budget plus the daemon's pre-auth delay, which PAM also blocks through, so it must be
/// added rather than absorbed. Assumes the resumed delay, which PAM cannot predict.
pub fn camera_auth_timeout(
    auth: &gaze_core::config::AuthConfig,
    service: Option<&str>,
) -> std::time::Duration {
    let surface = gaze_core::config::classify_pam_service(service);
    std::time::Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS)
        + std::time::Duration::from_millis(auth.effective_start_delay_ms(true, surface))
}
const CONFIRMATION_PROMPT: &str = "Face Verified. Press Enter to confirm, Esc to cancel.";
pub const CONFIRMATION_REQUEST: &str = "GAZE_CONFIRMATION_REQUEST";
pub const CONFIRMATION_ACK: &str = "CONFIRM";

pub const LOOK_PROMPT: &str = "Please look at the camera";
pub const LOOK_OR_PASSWORD_PROMPT: &str = "Please look at the camera or enter password";
pub const FACE_VERIFIED: &str = "Face Verified.";
pub const FACE_NOT_RECOGNIZED: &str = "Face not recognized. Enter your password.";
pub const FACE_NOT_DETECTED: &str = "Face not detected. Enter your password.";
pub const FACE_TOO_DARK: &str = "Too dark for face authentication. Enter your password.";
pub const FACE_TIMED_OUT: &str = "Face authentication timed out. Enter your password.";
pub const FACE_UNAVAILABLE: &str = "Face authentication unavailable. Enter your password.";

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

/// Under `PAM_SILENT` no camera prompt was printed, so clearing the line above would erase
/// unrelated terminal output instead of the line this message replaces.
fn line_prefix(silent: bool) -> &'static str {
    if silent { "\r" } else { "\x1B[1A\x1B[2K\r" }
}

fn replace_previous_line(
    writer: &mut impl Write,
    silent: bool,
    message: &str,
) -> std::io::Result<()> {
    write!(writer, "{}{message}", line_prefix(silent))
}

fn report_face_verified_to_tty() -> Option<()> {
    let mut tty = open_interactive_tty()?;
    replace_previous_line(&mut tty, false, FACE_VERIFIED).ok()?;
    writeln!(tty).ok()?;
    tty.flush().ok()
}

/// Replace the camera prompt with a non-interactive success message when a terminal is available.
/// Graphical PAM clients receive the same message through their conversation function instead.
pub unsafe fn report_face_verified(pamh: PamHandle, silent: bool) {
    if silent {
        return;
    }
    if report_face_verified_to_tty().is_none() {
        unsafe { say(pamh, FACE_VERIFIED) };
    }
}

fn confirm_from_tty(silent: bool) -> Option<bool> {
    let mut tty = open_interactive_tty()?;
    let fd = tty.as_raw_fd();

    let mut original = MaybeUninit::<libc::termios>::uninit();
    unsafe {
        if libc::tcgetattr(fd, original.as_mut_ptr()) != 0 {
            return None;
        }
        let original = original.assume_init();
        let mut raw = original;
        raw.c_lflag &= !(libc::ICANON | libc::ECHO);
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = TTY_CONFIRM_DECISECONDS;
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return None;
        }

        let _guard = TermiosGuard { fd, original };
        replace_previous_line(&mut tty, silent, CONFIRMATION_PROMPT).ok()?;
        tty.flush().ok()?;

        let mut key = [0_u8; 1];
        let read = tty.read(&mut key).ok()?;
        writeln!(tty).ok()?;
        Some(tty_confirmation(read, key[0]))
    }
}

/// A zero-length read is `VTIME` expiring, which is the user declining to confirm. Reporting it as
/// "no terminal" instead would re-prompt through PAM and wait for an answer with no deadline left.
fn tty_confirmation(read: usize, key: u8) -> bool {
    read != 0 && matches!(key, b'\n' | b'\r')
}
fn stdin_is_terminal() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 }
}

fn open_interactive_tty() -> Option<std::fs::File> {
    if !stdin_is_terminal() {
        return None;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()
}

pub fn has_interactive_tty() -> bool {
    open_interactive_tty().is_some()
}

pub unsafe fn confirm_authentication(pamh: PamHandle, silent: bool) -> bool {
    if let Some(confirmed) = confirm_from_tty(silent) {
        return confirmed;
    }

    unsafe { converse(pamh, PAM_PROMPT_ECHO_ON, CONFIRMATION_PROMPT) }
        .map(|resp| resp.is_empty())
        .unwrap_or(false)
}

pub fn confirmation_accepted(response: Option<&str>) -> bool {
    matches!(response, Some(CONFIRMATION_ACK))
}

pub fn confirmation_required(
    auth: Option<&gaze_core::config::AuthConfig>,
    service: Option<&str>,
) -> bool {
    let surface = gaze_core::config::classify_pam_service(service);
    auth.is_none_or(|auth| auth.requires_confirmation(surface))
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

pub fn give_up_message(status: Option<gaze_core::dbus::CaptureStatus>) -> &'static str {
    match status {
        Some(gaze_core::dbus::CaptureStatus::TooDark) => FACE_TOO_DARK,
        Some(gaze_core::dbus::CaptureStatus::NoFace) | None => FACE_NOT_DETECTED,
        Some(gaze_core::dbus::CaptureStatus::Unused) => FACE_UNAVAILABLE,
        _ => FACE_NOT_RECOGNIZED,
    }
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

// Like `active_or_user_uid`, but also flags a login greeter (e.g. GDM). A greeter always runs
// GNOME, yet its transient processes defeat `/proc` DE detection, so callers gate on this.
pub async fn active_confirm_target(username: &str) -> (Option<u32>, bool) {
    match gaze_core::dbus::get_active_session_uid_and_class().await {
        Ok((uid, class)) => (Some(uid), class == "greeter"),
        Err(_) => (get_user_uid(username), false),
    }
}

pub async fn gnome_extension_active_on(proxy: &GazeProxy<'_>, uid: Option<u32>) -> bool {
    let Some(uid) = uid else {
        return false;
    };
    proxy.is_extension_active(uid).await.unwrap_or(false)
}

pub async fn gnome_extension_active(uid: Option<u32>) -> bool {
    if uid.is_none() {
        return false;
    }
    match setup_auth_env().await {
        Ok((_config, proxy)) => gnome_extension_active_on(&proxy, uid).await,
        Err(_) => false,
    }
}

pub unsafe fn say(pamh: PamHandle, text: &str) {
    unsafe {
        let _ = converse(pamh, PAM_TEXT_INFO, text);
    }
}

pub unsafe fn warn(pamh: PamHandle, text: &str) {
    unsafe {
        let _ = converse(pamh, PAM_ERROR_MSG, text);
    }
}

pub unsafe fn report(pamh: PamHandle, service: Option<&str>, text: &str) {
    if service_shows_only_error_messages(service) {
        unsafe { warn(pamh, text) }
    } else {
        unsafe { say(pamh, text) }
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
    let config = match gaze_core::dbus::try_load_config_from_daemon(&proxy).await {
        Ok(Some(config)) => config,
        Ok(None) => {
            gaze_core::config::Config::load_from(gaze_core::config::CONFIG_PATH).unwrap_or_default()
        }
        Err(_) => return Err(PAM_SERVICE_ERR),
    };
    Ok((config, proxy))
}

pub async fn has_enrolled_faces_on(proxy: &GazeProxy<'_>, username: &str) -> anyhow::Result<bool> {
    match proxy.list_faces(username).await {
        // Treat unenrolled users as having no faces.
        Ok(faces) => Ok(!faces.is_empty()),
        Err(ref err) if gaze_core::dbus::dbus_is_file_not_found(err) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

pub async fn has_enrolled_faces(username: &str) -> anyhow::Result<bool> {
    let (_config, proxy) = setup_auth_env()
        .await
        .map_err(|e| anyhow::anyhow!("PAM error: {}", e))?;
    has_enrolled_faces_on(&proxy, username).await
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

/// The verdict's own view of the capture, derived the way the daemon derives `FaceStatus`.
/// That signal can be stale, missing on runs that never looked, or reached second by `select!`.
fn decisive_status(
    rgb_status: gaze_core::dbus::CaptureStatus,
    ir_status: gaze_core::dbus::CaptureStatus,
) -> Option<gaze_core::dbus::CaptureStatus> {
    Some(if rgb_status.priority() >= ir_status.priority() {
        rgb_status
    } else {
        ir_status
    })
}

fn auth_outcome(
    result: gaze_core::dbus::VerifyResult,
    last_status: Option<gaze_core::dbus::CaptureStatus>,
) -> AuthOutcome {
    match result {
        gaze_core::dbus::VerifyResult::VerifyMatch => AuthOutcome::Match,
        gaze_core::dbus::VerifyResult::VerifyNoMatch => match last_status {
            // `Unused` means the attempt was abandoned before it could decide, so it is not a
            // rejection and must not be reported as one.
            Some(
                gaze_core::dbus::CaptureStatus::TooDark
                | gaze_core::dbus::CaptureStatus::NoFace
                | gaze_core::dbus::CaptureStatus::Unused,
            ) => AuthOutcome::Unavailable,
            Some(status) if status.is_framing_hint() => AuthOutcome::Unavailable,
            _ => AuthOutcome::NoMatch,
        },
    }
}

async fn request_verify_start(
    proxy: &GazeProxy<'static>,
    service: Option<&str>,
) -> anyhow::Result<()> {
    match proxy
        .verify_start_for("any", service.unwrap_or_default())
        .await
    {
        Err(zbus::Error::MethodError(ref name, ..))
            if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" =>
        {
            proxy
                .verify_start("any")
                .await
                .map_err(|e| anyhow::anyhow!("Verify start failed: {}", e))
        }
        other => other.map_err(|e| anyhow::anyhow!("Verify start failed: {}", e)),
    }
}

pub async fn authenticate_biometric(
    username: &str,
    service: Option<&str>,
) -> anyhow::Result<AuthOutcome> {
    Ok(authenticate_biometric_with_status(username, service)
        .await?
        .0)
}

pub async fn authenticate_biometric_with_status(
    username: &str,
    service: Option<&str>,
) -> anyhow::Result<(AuthOutcome, Option<gaze_core::dbus::CaptureStatus>)> {
    let (_config, proxy) = setup_auth_env()
        .await
        .map_err(|e| anyhow::anyhow!("PAM error: {}", e))?;
    authenticate_biometric_with_status_on(&proxy, username, service).await
}

pub async fn authenticate_biometric_with_status_on(
    proxy: &GazeProxy<'static>,
    username: &str,
    service: Option<&str>,
) -> anyhow::Result<(AuthOutcome, Option<gaze_core::dbus::CaptureStatus>)> {
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
    request_verify_start(proxy, service).await?;

    use futures::StreamExt;
    let mut last_status: Option<gaze_core::dbus::CaptureStatus> = None;
    let outcome = loop {
        tokio::select! {
            Some(signal) = verify_stream.next() => {
                if let Ok(args) = signal.args() {
                    last_status = decisive_status(*args.rgb_status(), *args.ir_status());
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
    Ok((outcome, last_status))
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

pub fn service_defers_to_face_service(service: Option<&str>) -> bool {
    match service {
        Some(name) => name.starts_with("gdm-") && name != FACE_PAM_SERVICE,
        None => false,
    }
}

/// The two noninteractive slots KScreenLocker starts up front, either of which can hold Gaze.
/// `kde-smartcard` is used when a fingerprint reader already owns `kde-fingerprint`.
pub const KDE_FACE_PAM_SERVICE: &str = "kde-fingerprint";
pub const KDE_FACE_PAM_FILE: &str = "/etc/pam.d/kde-fingerprint";
pub const KDE_SMARTCARD_PAM_SERVICE: &str = "kde-smartcard";
pub const KDE_SMARTCARD_PAM_FILE: &str = "/etc/pam.d/kde-smartcard";

/// Plasma Login Manager's biometric helper, which runs alongside the password field instead
/// of after it. Gaze is wired into it only on distros that ship the service.
pub const PLASMALOGIN_FACE_PAM_SERVICE: &str = "plasmalogin-fingerprint";
pub const PLASMALOGIN_FACE_PAM_FILE: &str = "/etc/pam.d/plasmalogin-fingerprint";

/// The interactive services driving the password field, which reach Gaze through
/// the shared stack it installs into.
const KDE_INTERACTIVE_SERVICE: &str = "kde";
const PLASMALOGIN_INTERACTIVE_SERVICE: &str = "plasmalogin";

fn is_kde_noninteractive_service(service: Option<&str>) -> bool {
    matches!(
        service,
        Some(KDE_FACE_PAM_SERVICE | KDE_SMARTCARD_PAM_SERVICE)
    )
}

/// A slot the greeter starts by itself, with nothing to route a response back.
fn is_unpromptable_slot(service: Option<&str>) -> bool {
    is_kde_noninteractive_service(service) || service == Some(PLASMALOGIN_FACE_PAM_SERVICE)
}

pub fn pam_stack_runs_gaze(contents: Option<&str>) -> bool {
    contents.is_some_and(|text| text.lines().any(pam_auth_line_runs_gaze))
}

const GAZE_PAM_MODULES: [&str; 2] = ["pam_gaze.so", "pam_gaze_grosshack.so"];

fn pam_auth_line_runs_gaze(line: &str) -> bool {
    let line = line.split('#').next().unwrap_or_default();
    let mut fields = line.split_whitespace();
    if !matches!(fields.next(), Some("auth") | Some("-auth")) {
        return false;
    }
    fields.any(|field| {
        GAZE_PAM_MODULES.iter().any(|module| {
            // NixOS writes an absolute store path, not a bare module name.
            field == *module || field.ends_with(&format!("/{module}"))
        })
    })
}

fn pam_service_contents(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Which up-front slots a service must stand down for, so one unlock claims the camera once.
/// Plasma runs several PAM services per unlock, and the password-side ones also reach Gaze.
fn face_slots_outranking(service: Option<&str>) -> &'static [&'static str] {
    match service {
        Some(KDE_INTERACTIVE_SERVICE) => &[KDE_FACE_PAM_FILE, KDE_SMARTCARD_PAM_FILE],
        Some(KDE_SMARTCARD_PAM_SERVICE) => &[KDE_FACE_PAM_FILE],
        Some(PLASMALOGIN_INTERACTIVE_SERVICE) => &[PLASMALOGIN_FACE_PAM_FILE],
        _ => &[],
    }
}

pub fn service_defers_to_face_slot(service: Option<&str>) -> bool {
    face_slots_outranking(service)
        .iter()
        .any(|path| pam_stack_runs_gaze(pam_service_contents(path).as_deref()))
}

/// Nothing routes a response here, so a prompt wedges the slot for the lock.
pub fn service_cannot_be_prompted(service: Option<&str>) -> bool {
    is_unpromptable_slot(service)
}

/// The lock screen renders `PAM_ERROR_MSG` but discards `PAM_TEXT_INFO`.
pub fn service_shows_only_error_messages(service: Option<&str>) -> bool {
    is_kde_noninteractive_service(service)
}

/// These greeters allow one `pam_authenticate` per arming, so retry inside it.
pub fn service_retries_transient_give_up(service: Option<&str>) -> bool {
    is_unpromptable_slot(service)
}

const GNOME_BINARIES: [&str; 1] = ["gnome-shell"];
const KDE_BINARIES: [&str; 5] = [
    "plasmashell",
    "kwin_wayland",
    "kwin_x11",
    "lxqt-policykit-agent",
    "lxqt-policykit",
];
const HYPRLAND_BINARIES: [&str; 2] = ["hyprland", "Hyprland"];

pub fn binary_is_trusted(owner_uid: u32, mode: u32) -> bool {
    owner_uid == 0 && mode & 0o022 == 0
}

pub fn system_binary_path(link: &str) -> &str {
    link.strip_suffix(" (deleted)").unwrap_or(link)
}

pub fn desktop_from_binaries<I: IntoIterator<Item = String>>(names: I) -> String {
    let mut is_gnome = false;
    let mut is_kde = false;
    let mut is_hyprland = false;

    for name in names {
        let name = name.as_str();
        if GNOME_BINARIES.contains(&name) {
            is_gnome = true;
        } else if KDE_BINARIES.contains(&name) {
            is_kde = true;
        } else if HYPRLAND_BINARIES.contains(&name) {
            is_hyprland = true;
        }
    }

    if is_gnome {
        "GNOME".to_string()
    } else if is_kde {
        "KDE".to_string()
    } else if is_hyprland {
        "Hyprland".to_string()
    } else {
        "Other".to_string()
    }
}

fn trusted_binary_name(proc_entry: &std::path::Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;

    let link = std::fs::read_link(proc_entry.join("exe")).ok()?;
    let exe = std::path::Path::new(system_binary_path(link.to_str()?));
    let metadata = std::fs::metadata(exe).ok()?;
    if !binary_is_trusted(metadata.uid(), metadata.mode()) {
        return None;
    }
    Some(exe.file_name()?.to_str()?.to_string())
}

pub fn detect_desktop_environment(uid: u32) -> String {
    use std::os::unix::fs::MetadataExt;

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return "Other".to_string();
    };

    let names = entries.flatten().filter_map(|entry| {
        let metadata = entry.metadata().ok()?;
        if !metadata.is_dir() || metadata.uid() != uid {
            return None;
        }
        let path = entry.path();
        let pid = path.file_name()?.to_str()?;
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        trusted_binary_name(&path)
    });

    desktop_from_binaries(names)
}

#[derive(Debug, PartialEq, Eq)]
pub enum GraphicalConfirm {
    GnomeExtension,
    FailClosed,
}

/// No channel to confirm through means the match is refused, not granted.
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
    // Not Bypass: slots that genuinely cannot answer a prompt are handled by
    // `service_cannot_be_prompted` first, so bypassing would only weaken hyprlock and friends.
    match de {
        "GNOME" if extension_active => GraphicalConfirm::GnomeExtension,
        _ => GraphicalConfirm::FailClosed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // The escape moves up a line and clears it, so it must only run when a line was printed.
    #[test]
    fn a_silent_prompt_does_not_clear_the_line_above_it() {
        assert_eq!(line_prefix(false), "\x1B[1A\x1B[2K\r");
        assert_eq!(line_prefix(true), "\r");
    }

    #[test]
    fn a_silent_confirmation_prompt_is_still_written() {
        let mut out = Vec::new();
        replace_previous_line(&mut out, true, CONFIRMATION_PROMPT).unwrap();
        let written = String::from_utf8(out).unwrap();
        assert!(written.contains(CONFIRMATION_PROMPT));
        assert!(!written.contains("\x1B["));
    }

    #[test]
    fn enter_confirms_and_any_other_key_declines() {
        assert!(tty_confirmation(1, b'\n'));
        assert!(tty_confirmation(1, b'\r'));
        assert!(!tty_confirmation(1, 0x1b));
        assert!(!tty_confirmation(1, b'x'));
    }

    // A timeout must decline outright; treating it as an absent terminal re-prompted unbounded.
    #[test]
    fn an_unanswered_prompt_declines_rather_than_reprompting() {
        assert!(!tty_confirmation(0, b'\n'));
        assert!(!tty_confirmation(0, 0));
    }

    #[test]
    fn only_root_owned_unwritable_binaries_are_trusted() {
        assert!(binary_is_trusted(0, 0o755));
        assert!(binary_is_trusted(0, 0o555));
        assert!(!binary_is_trusted(1000, 0o755));
        assert!(!binary_is_trusted(0, 0o775));
        assert!(!binary_is_trusted(0, 0o777));
    }

    #[test]
    fn a_replaced_binary_still_resolves_to_its_path() {
        assert_eq!(
            system_binary_path("/usr/bin/gnome-shell (deleted)"),
            "/usr/bin/gnome-shell"
        );
        assert_eq!(
            system_binary_path("/usr/bin/gnome-shell"),
            "/usr/bin/gnome-shell"
        );
    }

    #[test]
    fn each_desktop_is_recognised_by_its_binary() {
        assert_eq!(desktop_from_binaries(names(&["gnome-shell"])), "GNOME");
        assert_eq!(desktop_from_binaries(names(&["plasmashell"])), "KDE");
        assert_eq!(desktop_from_binaries(names(&["Hyprland"])), "Hyprland");
        assert_eq!(desktop_from_binaries(names(&["sleep", "bash"])), "Other");
    }

    #[test]
    fn a_long_binary_name_is_no_longer_truncated_away() {
        assert_eq!(
            desktop_from_binaries(names(&["lxqt-policykit-agent"])),
            "KDE"
        );
    }

    #[test]
    fn an_ambiguous_session_takes_the_strictest_desktop() {
        assert_eq!(
            desktop_from_binaries(names(&["plasmashell", "gnome-shell"])),
            "GNOME"
        );
        assert_eq!(
            desktop_from_binaries(names(&["Hyprland", "gnome-shell"])),
            "GNOME"
        );
    }

    #[test]
    fn face_slots_are_outranked_in_one_direction_only() {
        assert_eq!(
            face_slots_outranking(Some("kde")),
            [KDE_FACE_PAM_FILE, KDE_SMARTCARD_PAM_FILE],
            "the password field must yield to either biometric slot"
        );
        assert_eq!(
            face_slots_outranking(Some(KDE_SMARTCARD_PAM_SERVICE)),
            [KDE_FACE_PAM_FILE],
            "the smartcard slot yields to the fingerprint slot, never the reverse"
        );
        assert_eq!(
            face_slots_outranking(Some("plasmalogin")),
            [PLASMALOGIN_FACE_PAM_FILE],
            "one submit must not run face auth in both greeter helpers"
        );
        for service in [
            Some(KDE_FACE_PAM_SERVICE),
            Some(PLASMALOGIN_FACE_PAM_SERVICE),
            // sddm has no up-front helper to yield to.
            Some("sddm"),
            None,
        ] {
            assert!(
                face_slots_outranking(service).is_empty(),
                "{service:?} must never stand down"
            );
        }
    }

    #[test]
    fn gaze_is_detected_on_an_auth_line_only() {
        assert!(pam_stack_runs_gaze(Some(
            "auth        [success=done default=ignore]    pam_gaze.so"
        )));
        assert!(pam_stack_runs_gaze(Some(
            "auth sufficient pam_gaze_grosshack.so"
        )));
        assert!(pam_stack_runs_gaze(Some(
            "#%PAM-1.0\nauth required pam_fprintd.so\nauth sufficient pam_gaze.so"
        )));
        assert!(pam_stack_runs_gaze(Some(
            "auth [success=done default=ignore] /nix/store/abc123-gaze-0.2.7/lib/security/pam_gaze.so"
        )));
        assert!(pam_stack_runs_gaze(Some("-auth optional pam_gaze.so")));

        assert!(!pam_stack_runs_gaze(Some("# auth sufficient pam_gaze.so")));
        assert!(!pam_stack_runs_gaze(Some(
            "auth required pam_fprintd.so # not pam_gaze.so"
        )));
        assert!(!pam_stack_runs_gaze(Some("session optional pam_gaze.so")));
        assert!(!pam_stack_runs_gaze(Some(
            "auth optional pam_gaze_other.so"
        )));
        assert!(!pam_stack_runs_gaze(Some("auth required pam_fprintd.so")));
        assert!(!pam_stack_runs_gaze(None));
    }

    #[test]
    fn only_greeter_started_slots_are_treated_as_unpromptable() {
        for slot in [
            KDE_FACE_PAM_SERVICE,
            KDE_SMARTCARD_PAM_SERVICE,
            PLASMALOGIN_FACE_PAM_SERVICE,
        ] {
            assert!(service_cannot_be_prompted(Some(slot)), "{slot}");
            assert!(service_retries_transient_give_up(Some(slot)), "{slot}");
        }

        // Discarding info messages is a KScreenLocker theme quirk, not a PLM one.
        for slot in [KDE_FACE_PAM_SERVICE, KDE_SMARTCARD_PAM_SERVICE] {
            assert!(service_shows_only_error_messages(Some(slot)), "{slot}");
        }
        assert!(!service_shows_only_error_messages(Some(
            PLASMALOGIN_FACE_PAM_SERVICE
        )));

        for service in [
            "kde",
            "hyprlock-gaze",
            "gdm-face",
            "sudo",
            "polkit-1",
            "plasmalogin",
        ] {
            assert!(!service_cannot_be_prompted(Some(service)), "{service}");
            assert!(
                !service_shows_only_error_messages(Some(service)),
                "{service}"
            );
            assert!(
                !service_retries_transient_give_up(Some(service)),
                "{service}"
            );
        }
        assert!(!service_cannot_be_prompted(None));
        assert!(!service_shows_only_error_messages(None));
        assert!(!service_retries_transient_give_up(None));
    }

    #[test]
    fn other_desktops_fail_closed_without_a_channel() {
        assert_eq!(
            graphical_confirm_decision("GNOME", true, false),
            GraphicalConfirm::GnomeExtension
        );
        assert_eq!(
            graphical_confirm_decision("GNOME", false, false),
            GraphicalConfirm::FailClosed
        );
        for de in ["KDE", "Hyprland", "LXQt", "Other"] {
            assert_eq!(
                graphical_confirm_decision(de, false, false),
                GraphicalConfirm::FailClosed,
                "{de} has no confirm channel and must fail closed, not bypass"
            );
        }
    }

    #[test]
    fn an_unpromptable_slot_never_reaches_the_graphical_decision() {
        // Why failing closed above is safe: the KDE lock screen slots return
        // before a confirmation is ever attempted.
        for slot in [
            KDE_FACE_PAM_SERVICE,
            KDE_SMARTCARD_PAM_SERVICE,
            PLASMALOGIN_FACE_PAM_SERVICE,
        ] {
            assert!(service_cannot_be_prompted(Some(slot)), "{slot}");
        }
    }

    #[test]
    fn a_greeter_never_bypasses_confirmation() {
        assert_eq!(
            graphical_confirm_decision("Other", true, true),
            GraphicalConfirm::GnomeExtension
        );
        for de in ["GNOME", "KDE", "Hyprland", "Other"] {
            assert_eq!(
                graphical_confirm_decision(de, false, true),
                GraphicalConfirm::FailClosed,
                "a greeter ({de}) must fail closed rather than bypass"
            );
        }
    }

    #[test]
    fn pre_auth_delay_extends_the_camera_budget_instead_of_consuming_it() {
        let mut auth = gaze_core::config::AuthConfig::default();
        let base = std::time::Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS);

        assert_eq!(camera_auth_timeout(&auth, Some("hyprlock-gaze")), base);

        auth.start_delay_ms = 5000;
        assert_eq!(
            camera_auth_timeout(&auth, Some("hyprlock-gaze")),
            base + std::time::Duration::from_millis(5000)
        );

        // The daemon waits for whichever delay is longer, so budget for that.
        auth.resume_grace_ms = 9000;
        assert_eq!(
            camera_auth_timeout(&auth, Some("hyprlock-gaze")),
            base + std::time::Duration::from_millis(9000)
        );

        auth.start_delay_ms = 0;
        assert_eq!(
            camera_auth_timeout(&auth, Some("hyprlock-gaze")),
            base + std::time::Duration::from_millis(9000)
        );
    }

    #[test]
    fn scoped_away_prompts_keep_the_plain_camera_budget() {
        let base = std::time::Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS);
        let auth = gaze_core::config::AuthConfig {
            start_delay_ms: 5000,
            start_delay_scope: "screen_lock".to_string(),
            ..Default::default()
        };

        assert_eq!(camera_auth_timeout(&auth, Some("sudo")), base);
        assert_eq!(
            camera_auth_timeout(&auth, Some("hyprlock-gaze")),
            base + std::time::Duration::from_millis(5000)
        );
    }

    #[test]
    fn gdm_services_other_than_face_defer() {
        for service in ["gdm-password", "gdm-fingerprint", "gdm-launch-environment"] {
            assert!(
                service_defers_to_face_service(Some(service)),
                "{service} must defer to gdm-face"
            );
        }
    }

    #[test]
    fn face_service_and_non_gdm_services_run() {
        for service in [
            "gdm-face",
            "polkit-1",
            "sudo",
            "login",
            "su",
            "hyprlock-gaze",
            "sddm",
        ] {
            assert!(
                !service_defers_to_face_service(Some(service)),
                "{service} must still run face auth"
            );
        }
        assert!(!service_defers_to_face_service(None));
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
    fn confirmation_is_required_when_the_config_could_not_be_read() {
        assert!(confirmation_required(None, None));
        assert!(confirmation_required(None, Some("sudo")));
        assert!(confirmation_required(None, Some("swaylock")));
    }

    #[test]
    fn confirmation_follows_the_lock_screen_toggle() {
        let off = gaze_core::config::AuthConfig {
            require_confirmation_lock_screen: false,
            ..Default::default()
        };
        assert!(!confirmation_required(Some(&off), Some("swaylock")));
        assert!(!confirmation_required(Some(&off), Some("gdm-password")));

        let on = gaze_core::config::AuthConfig {
            require_confirmation_lock_screen: true,
            ..Default::default()
        };
        assert!(confirmation_required(Some(&on), Some("swaylock")));
        assert!(confirmation_required(Some(&on), Some("gdm-password")));
    }

    #[test]
    fn confirmation_follows_the_elevation_toggle_independently() {
        let lock_only = gaze_core::config::AuthConfig {
            require_confirmation_lock_screen: true,
            require_confirmation_elevation: false,
            ..Default::default()
        };
        assert!(confirmation_required(Some(&lock_only), Some("swaylock")));
        assert!(!confirmation_required(Some(&lock_only), Some("sudo")));

        let elevation_only = gaze_core::config::AuthConfig {
            require_confirmation_lock_screen: false,
            require_confirmation_elevation: true,
            ..Default::default()
        };
        assert!(!confirmation_required(
            Some(&elevation_only),
            Some("swaylock")
        ));
        assert!(confirmation_required(Some(&elevation_only), Some("sudo")));
    }

    #[test]
    fn direct_callers_never_require_confirmation() {
        let both_on = gaze_core::config::AuthConfig {
            require_confirmation_lock_screen: true,
            require_confirmation_elevation: true,
            ..Default::default()
        };
        assert!(!confirmation_required(Some(&both_on), None));
        assert!(!confirmation_required(Some(&both_on), Some("")));
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
    fn only_the_silent_flag_silences_the_module() {
        assert!(caller_wants_silence(PAM_SILENT));
        assert!(caller_wants_silence(PAM_SILENT | PAM_DISALLOW_NULL_AUTHTOK));
        assert!(!caller_wants_silence(0));
        assert!(!caller_wants_silence(PAM_DISALLOW_NULL_AUTHTOK));
    }

    #[test]
    fn face_verified_replaces_the_previous_terminal_prompt() {
        let mut output = Vec::new();

        replace_previous_line(&mut output, false, FACE_VERIFIED).unwrap();

        assert_eq!(
            String::from_utf8(output).unwrap(),
            "\x1B[1A\x1B[2K\rFace Verified."
        );
        assert!(!FACE_VERIFIED.contains("confirm"));
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
    fn give_up_messages_never_repeat_the_opening_prompt() {
        use gaze_core::dbus::CaptureStatus;

        for status in [
            Some(CaptureStatus::NoFace),
            Some(CaptureStatus::TooDark),
            Some(CaptureStatus::Usable),
            Some(CaptureStatus::Unused),
            None,
        ] {
            let message = give_up_message(status);
            assert!(
                !message.starts_with("Please look at the camera"),
                "{status:?}"
            );
            assert!(message.contains("password"), "{status:?}");
        }
    }

    #[test]
    fn give_up_message_keeps_the_actionable_cause() {
        use gaze_core::dbus::CaptureStatus;

        assert_eq!(give_up_message(Some(CaptureStatus::TooDark)), FACE_TOO_DARK);
        assert_eq!(
            give_up_message(Some(CaptureStatus::NoFace)),
            FACE_NOT_DETECTED
        );
        assert_eq!(give_up_message(None), FACE_NOT_DETECTED);
        assert_eq!(
            give_up_message(Some(CaptureStatus::Usable)),
            FACE_NOT_RECOGNIZED
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

    #[test]
    fn a_cancelled_attempt_is_unavailable_rather_than_a_rejection() {
        use gaze_core::dbus::{CaptureStatus, VerifyResult};

        // A preempted claim reports an idle camera; treating that as a rejection would count
        // an attempt the user never made toward lockout.
        assert_eq!(
            auth_outcome(VerifyResult::VerifyNoMatch, Some(CaptureStatus::Unused)),
            AuthOutcome::Unavailable
        );
    }

    #[test]
    fn the_verdict_decides_what_the_camera_saw() {
        use gaze_core::dbus::{CaptureStatus, VerifyResult};

        // The higher-priority spectrum wins, matching how the daemon picks the status it
        // reports, so the two cannot disagree.
        assert_eq!(
            decisive_status(CaptureStatus::Unused, CaptureStatus::Usable),
            Some(CaptureStatus::Usable)
        );
        assert_eq!(
            decisive_status(CaptureStatus::Usable, CaptureStatus::Unused),
            Some(CaptureStatus::Usable)
        );
        assert_eq!(
            decisive_status(CaptureStatus::NoFace, CaptureStatus::TooDark),
            Some(CaptureStatus::TooDark)
        );

        // Every way a run can end without judging a frame. Neither is a rejection, and no
        // `FaceStatus` is emitted on either path to say so.
        for (rgb, ir) in [
            (CaptureStatus::Unused, CaptureStatus::Unused),
            (CaptureStatus::NoFace, CaptureStatus::NoFace),
        ] {
            assert_eq!(
                auth_outcome(VerifyResult::VerifyNoMatch, decisive_status(rgb, ir)),
                AuthOutcome::Unavailable,
                "{rgb:?}/{ir:?} must fall through to the password, not count as a failure"
            );
        }

        // A spectrum that did judge a face still produces a rejection that counts.
        assert_eq!(
            auth_outcome(
                VerifyResult::VerifyNoMatch,
                decisive_status(CaptureStatus::Usable, CaptureStatus::Unused)
            ),
            AuthOutcome::NoMatch
        );
    }

    #[test]
    fn a_mis_framed_face_does_not_count_as_a_failed_attempt() {
        use gaze_core::dbus::{CaptureStatus, VerifyResult};

        for status in [
            CaptureStatus::Clipped,
            CaptureStatus::NotCentered,
            CaptureStatus::TooFar,
            CaptureStatus::TooClose,
        ] {
            assert_eq!(
                auth_outcome(VerifyResult::VerifyNoMatch, Some(status)),
                AuthOutcome::Unavailable,
                "{status:?} must fall through to the password, not count as a failure"
            );
        }
    }
}
