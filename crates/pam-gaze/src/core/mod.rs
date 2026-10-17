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

pub use gaze_core::desktop::{
    KDE_FACE_PAM_FILE, KDE_SMARTCARD_PAM_FILE, PLASMALOGIN_FACE_PAM_FILE,
};

use gaze_core::config::Config;
use zeroize::{Zeroize, Zeroizing};

mod service;

use service::is_kde_login_greeter;
pub use service::{
    KDE_FACE_PAM_SERVICE, KDE_SMARTCARD_PAM_SERVICE, OMARCHY_FACE_PAM_FILE,
    OMARCHY_FACE_PAM_SERVICE, PLASMALOGIN_FACE_PAM_SERVICE, caller_is_remote,
    face_auth_out_of_scope, pam_stack_runs_gaze, root_service_is_local, service_cannot_be_prompted,
    service_defers_to_face_service, service_defers_to_face_slot, service_is_network_facing,
    service_retries_transient_give_up, service_shows_only_error_messages,
};

/// Sensitive PAM responses (passwords). Wiped on drop.
pub type SecretString = Zeroizing<String>;
/// A password exactly as typed, which need not be valid UTF-8 in a non-UTF-8 locale.
pub type SecretBytes = Zeroizing<Vec<u8>>;

pub const PAM_SUCCESS: c_int = 0;
pub const PAM_AUTH_ERR: c_int = 7;
pub const PAM_SERVICE_ERR: c_int = 3;
pub const PAM_CONV: c_int = 5;
pub const PAM_SERVICE: c_int = 1;
pub const PAM_TTY: c_int = 3;
pub const PAM_RHOST: c_int = 4;
pub const PAM_AUTHTOK: c_int = 6;
pub const PAM_TEXT_INFO: c_int = 4;
pub const PAM_ERROR_MSG: c_int = 3;
pub const PAM_PROMPT_ECHO_OFF: c_int = 1;
pub const PAM_PROMPT_ECHO_ON: c_int = 2;
pub const PAM_AUTHINFO_UNAVAIL: c_int = 9;
pub const PAM_IGNORE: c_int = 25;

pub const PAM_DISALLOW_NULL_AUTHTOK: c_int = 0x0001;
pub const PAM_SILENT: c_int = 0x8000;
pub const PAM_ESTABLISH_CRED: c_int = 0x0002;
pub const PAM_DELETE_CRED: c_int = 0x0004;
pub const PAM_REINITIALIZE_CRED: c_int = 0x0008;
pub const PAM_REFRESH_CRED: c_int = 0x0010;
pub const PAM_DATA_REPLACE: c_int = 0x2000_0000;
pub const PAM_DATA_SILENT: c_int = 0x4000_0000;

pub fn caller_wants_silence(flags: c_int) -> bool {
    flags & PAM_SILENT != 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptLine {
    Printed,
    Absent,
}

impl PromptLine {
    fn keeps_previous_line(self) -> bool {
        matches!(self, Self::Absent)
    }
}

pub const CAMERA_AUTH_TIMEOUT_SECS: u64 = 12;
pub const TTY_CONFIRM_DECISECONDS: libc::cc_t = 200;
const _: () = assert!(TTY_CONFIRM_DECISECONDS > 0);
pub const FACE_PAM_SERVICE: &str = "gdm-face";
pub const GREETD_PAM_SERVICE: &str = "greetd";

/// Includes both the camera budget and the daemon's pre-auth delay, since PAM waits through each.
/// Uses the resume delay as a conservative estimate because PAM cannot tell when resume is active.
pub fn camera_auth_timeout(
    auth: &gaze_core::config::AuthConfig,
    service: Option<&str>,
) -> std::time::Duration {
    let surface = gaze_core::config::classify_pam_service(service);
    std::time::Duration::from_secs(CAMERA_AUTH_TIMEOUT_SECS)
        + std::time::Duration::from_millis(auth.effective_start_delay_ms(true, surface))
}
pub const CONFIRMATION_PROMPT: &str = "Face Verified. Press Enter to confirm, Esc to cancel.";

pub const LOOK_PROMPT: &str = "Please look at the camera";
pub const LOOK_OR_PASSWORD_PROMPT: &str = "Please look at the camera or enter password";
pub const LOOK_AFTER_PASSWORD_PROMPT: &str = "Password incorrect. Please look at the camera";
pub const FACE_VERIFIED: &str = "Face Verified.";
pub const FACE_NOT_RECOGNIZED: &str = "Face not recognized. Enter your password.";
pub const FACE_NOT_DETECTED: &str = "Face not detected. Enter your password.";
pub const FACE_TOO_DARK: &str = "Too dark for face authentication. Enter your password.";
pub const FACE_TIMED_OUT: &str = "Face authentication timed out. Enter your password.";
pub const FACE_UNAVAILABLE: &str = "Face authentication unavailable. Enter your password.";

pub use gaze_core::dbus::{
    GAZE_CANCEL, GAZE_CONFIRMED, GAZE_MSG_FACE_NOT_DETECTED, GAZE_MSG_FACE_NOT_RECOGNIZED,
    GAZE_MSG_FACE_TIMED_OUT, GAZE_MSG_FACE_TOO_DARK, GAZE_MSG_FACE_UNAVAILABLE,
    GAZE_MSG_FACE_VERIFIED, GAZE_MSG_LOOK_CAMERA, GAZE_MSG_LOOK_OR_PASSWORD,
    GAZE_REQUIRE_CONFIRMATION,
};

fn pam_service_name(service: &str) -> &str {
    std::path::Path::new(service.trim())
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(service.trim())
}

pub fn is_service_internal(service: &str, internal_list: &[String]) -> bool {
    let service_name = pam_service_name(service);
    internal_list
        .iter()
        .any(|item| pam_service_name(item) == service_name)
}

pub fn internal_give_up_message(status: Option<gaze_core::dbus::CaptureStatus>) -> &'static str {
    match status {
        Some(gaze_core::dbus::CaptureStatus::TooDark) => GAZE_MSG_FACE_TOO_DARK,
        Some(gaze_core::dbus::CaptureStatus::NoFace) | None => GAZE_MSG_FACE_NOT_DETECTED,
        Some(gaze_core::dbus::CaptureStatus::Unused) => GAZE_MSG_FACE_UNAVAILABLE,
        _ => GAZE_MSG_FACE_NOT_RECOGNIZED,
    }
}

pub fn internal_confirmation_accepted(response: Option<&str>) -> bool {
    response.map(str::trim) == Some(GAZE_CONFIRMED)
}

pub fn internal_prompt_confirmation_accepted(response: Option<&str>) -> bool {
    let answer = response.map(str::trim);
    answer == Some(GAZE_CONFIRMED) || answer == Some("")
}

pub type PamHandle = *mut c_void;

/// A panic unwinding out of an `extern "C"` entry point aborts the host: sudo, the greeter or
/// the lock screen. Report it as a module failure so the stack falls through instead.
pub fn guard_pam_entry(entry: impl FnOnce() -> c_int) -> c_int {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(entry)).unwrap_or(PAM_SERVICE_ERR)
}

#[macro_export]
macro_rules! pam_success_stubs {
    () => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn pam_sm_setcred(
            pamh: $crate::PamHandle,
            flags: ::std::os::raw::c_int,
            _argc: ::std::os::raw::c_int,
            _argv: *const *const ::std::os::raw::c_char,
        ) -> ::std::os::raw::c_int {
            $crate::guard_pam_entry(|| {
                unsafe { $crate::clear_duress_after_credentials(pamh, flags) };
                $crate::PAM_SUCCESS
            })
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
    pub fn pam_set_data(
        pamh: PamHandle,
        module_data_name: *const c_char,
        data: *mut c_void,
        cleanup: Option<
            unsafe extern "C" fn(pamh: PamHandle, data: *mut c_void, error_status: c_int),
        >,
    ) -> c_int;
    pub fn pam_get_data(
        pamh: PamHandle,
        module_data_name: *const c_char,
        data: *mut *const c_void,
    ) -> c_int;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FirstPassVerdict {
    NoMatch = 1,
    Undecided = 2,
    Preempted = 3,
}

impl FirstPassVerdict {
    pub fn from_repr(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::NoMatch),
            2 => Some(Self::Undecided),
            3 => Some(Self::Preempted),
            _ => None,
        }
    }

    pub fn allows_retry(self) -> bool {
        !matches!(self, Self::NoMatch)
    }
}

pub const FIRST_PASS_DATA_KEY: &CStr = c"gaze_first_pass_verdict";

unsafe extern "C" fn free_first_pass_verdict(_pamh: PamHandle, data: *mut c_void, _status: c_int) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(data as *mut u8) });
    }
}

pub unsafe fn record_first_pass_verdict(pamh: PamHandle, verdict: FirstPassVerdict) {
    let boxed = Box::into_raw(Box::new(verdict as u8));
    let rc = unsafe {
        pam_set_data(
            pamh,
            FIRST_PASS_DATA_KEY.as_ptr(),
            boxed as *mut c_void,
            Some(free_first_pass_verdict),
        )
    };
    if rc != PAM_SUCCESS {
        drop(unsafe { Box::from_raw(boxed) });
    }
}

pub unsafe fn read_first_pass_verdict(pamh: PamHandle) -> Option<FirstPassVerdict> {
    let mut data: *const c_void = ptr::null();
    let rc = unsafe { pam_get_data(pamh, FIRST_PASS_DATA_KEY.as_ptr(), &mut data) };
    if rc != PAM_SUCCESS || data.is_null() {
        return None;
    }
    FirstPassVerdict::from_repr(unsafe { *(data as *const u8) })
}

const SCAN_FINISHED_DATA_KEY: &CStr = c"gaze_scan_finished";

/// Suppress duplicate Gaze entries in one stack pass, including shared-stack entries.
/// A new attempt on the same handle first waits for a typed password.
const SAME_PASS_WINDOW: std::time::Duration = std::time::Duration::from_secs(2);

unsafe extern "C" fn free_scan_finished(_pamh: PamHandle, data: *mut c_void, _status: c_int) {
    if !data.is_null() {
        drop(unsafe { Box::from_raw(data as *mut std::time::Instant) });
    }
}

pub unsafe fn record_scan_finished(pamh: PamHandle) {
    let boxed = Box::into_raw(Box::new(std::time::Instant::now()));
    let rc = unsafe {
        pam_set_data(
            pamh,
            SCAN_FINISHED_DATA_KEY.as_ptr(),
            boxed as *mut c_void,
            Some(free_scan_finished),
        )
    };
    if rc != PAM_SUCCESS {
        drop(unsafe { Box::from_raw(boxed) });
    }
}

pub unsafe fn scanned_in_this_pass(pamh: PamHandle) -> bool {
    let mut data: *const c_void = ptr::null();
    let rc = unsafe { pam_get_data(pamh, SCAN_FINISHED_DATA_KEY.as_ptr(), &mut data) };
    rc == PAM_SUCCESS
        && !data.is_null()
        && unsafe { *(data as *const std::time::Instant) }.elapsed() < SAME_PASS_WINDOW
}

pub const DURESS_CLEAR_DATA_KEY: &CStr = c"gaze_duress_clear";
const DURESS_CLEAR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

pub fn setcred_follows_authentication(flags: c_int) -> bool {
    flags & PAM_DELETE_CRED == 0
        && flags & (PAM_ESTABLISH_CRED | PAM_REINITIALIZE_CRED | PAM_REFRESH_CRED) != 0
}

pub fn pam_end_reports_success(status: c_int) -> bool {
    status & PAM_DATA_REPLACE == 0 && status & !PAM_DATA_SILENT == PAM_SUCCESS
}

pub fn clear_duress_lockout(username: &str) {
    // The daemon trusts a root host's PAM success. An unprivileged locker would only raise an
    // administrator challenge that nobody answers before the timeout, on every unlock.
    if unsafe { libc::geteuid() } != 0 {
        return;
    }
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    rt.block_on(async {
        let _ = tokio::time::timeout(DURESS_CLEAR_TIMEOUT, async {
            let proxy = gaze_core::dbus::connect_gaze().await?;
            if !proxy.duress_locked(username).await? {
                return Ok(false);
            }
            proxy.clear_duress(username).await
        })
        .await;
    });
    rt.shutdown_timeout(DURESS_CLEAR_TIMEOUT);
}

unsafe extern "C" fn clear_duress_on_pam_end(_pamh: PamHandle, data: *mut c_void, status: c_int) {
    if data.is_null() {
        return;
    }
    let username = unsafe { Box::from_raw(data as *mut String) };
    if pam_end_reports_success(status) {
        guard_pam_entry(|| {
            clear_duress_lockout(&username);
            PAM_SUCCESS
        });
    }
}

pub unsafe fn track_duress_clear(pamh: PamHandle, face_result: c_int) {
    if face_result == PAM_SUCCESS {
        unsafe { pam_set_data(pamh, DURESS_CLEAR_DATA_KEY.as_ptr(), ptr::null_mut(), None) };
        return;
    }
    let Some(username) = (unsafe { get_username(pamh) }) else {
        return;
    };
    let boxed = Box::into_raw(Box::new(username));
    let rc = unsafe {
        pam_set_data(
            pamh,
            DURESS_CLEAR_DATA_KEY.as_ptr(),
            boxed as *mut c_void,
            Some(clear_duress_on_pam_end),
        )
    };
    if rc != PAM_SUCCESS {
        drop(unsafe { Box::from_raw(boxed) });
    }
}

pub unsafe fn clear_duress_after_credentials(pamh: PamHandle, flags: c_int) {
    if !setcred_follows_authentication(flags) {
        return;
    }
    let mut data: *const c_void = ptr::null();
    if unsafe { pam_get_data(pamh, DURESS_CLEAR_DATA_KEY.as_ptr(), &mut data) } != PAM_SUCCESS
        || data.is_null()
    {
        return;
    }
    let username = unsafe { &*(data as *const String) }.clone();
    unsafe { pam_set_data(pamh, DURESS_CLEAR_DATA_KEY.as_ptr(), ptr::null_mut(), None) };
    clear_duress_lockout(&username);
}

/// Wipe a libc-allocated conversation response before freeing it, so a typed
/// password never lingers in freed heap memory.
unsafe fn free_conv_response(resp: *mut c_char) {
    if resp.is_null() {
        return;
    }
    unsafe {
        let len = libc::strlen(resp);
        if len > 0 {
            std::slice::from_raw_parts_mut(resp, len).zeroize();
        }
        libc::free(resp as *mut c_void);
    }
}

pub unsafe fn converse(pamh: PamHandle, msg_style: c_int, text: &str) -> Option<SecretString> {
    let mut bytes = unsafe { converse_bytes(pamh, msg_style, text) }?;
    match String::from_utf8(std::mem::take(&mut *bytes)) {
        Ok(text) => Some(Zeroizing::new(text)),
        Err(err) => {
            err.into_bytes().zeroize();
            None
        }
    }
}

pub unsafe fn converse_bytes(pamh: PamHandle, msg_style: c_int, text: &str) -> Option<SecretBytes> {
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

        let mut result: Option<SecretBytes> = None;
        if !resp_ptr.is_null() {
            let resp = (*resp_ptr).resp;
            if !resp.is_null() {
                result = Some(Zeroizing::new(CStr::from_ptr(resp).to_bytes().to_vec()));
                free_conv_response(resp);
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

fn line_prefix(prompt: PromptLine) -> &'static str {
    if prompt.keeps_previous_line() {
        "\r"
    } else {
        "\x1B[1A\x1B[2K\r"
    }
}

fn replace_previous_line(
    writer: &mut impl Write,
    prompt: PromptLine,
    message: &str,
) -> std::io::Result<()> {
    write!(writer, "{}{message}", line_prefix(prompt))
}

pub(crate) const TTY_CONVERSATION_SERVICES: [&str; 6] =
    ["sudo", "sudo-i", "su", "su-l", "doas", "login"];

/// Only these hosts read the terminal. A locker or greeter that merely inherited one would leave
/// anything written there unseen, and a key read from it would never come.
pub fn service_reads_the_tty(service: Option<&str>) -> bool {
    service.is_some_and(|s| TTY_CONVERSATION_SERVICES.contains(&s))
}

fn service_tty(service: Option<&str>) -> Option<std::fs::File> {
    if !service_reads_the_tty(service) {
        return None;
    }
    open_interactive_tty()
}

pub fn write_tty_line(service: Option<&str>, message: &str) -> Option<()> {
    let mut tty = service_tty(service)?;
    writeln!(tty, "{message}").ok()?;
    tty.flush().ok()
}

pub unsafe fn announce_prompt(
    pamh: PamHandle,
    service: Option<&str>,
    silent: bool,
    prompt: &str,
    is_internal: bool,
) -> PromptLine {
    if is_internal {
        if !silent {
            unsafe { say(pamh, prompt) };
        }
        return PromptLine::Absent;
    }
    if !silent {
        unsafe { say(pamh, prompt) };
        return PromptLine::Printed;
    }
    match write_tty_line(service, prompt) {
        Some(()) => PromptLine::Printed,
        None => PromptLine::Absent,
    }
}

fn report_face_verified_to_tty(service: Option<&str>, prompt: PromptLine) -> Option<()> {
    let mut tty = service_tty(service)?;
    replace_previous_line(&mut tty, prompt, FACE_VERIFIED).ok()?;
    writeln!(tty).ok()?;
    tty.flush().ok()
}

/// Replace the camera prompt with a non-interactive success message when a terminal is available.
/// Graphical PAM clients receive the same message through their conversation function instead.
pub unsafe fn report_face_verified(
    pamh: PamHandle,
    service: Option<&str>,
    silent: bool,
    prompt: PromptLine,
    is_internal: bool,
) {
    if !is_internal && report_face_verified_to_tty(service, prompt).is_some() {
        return;
    }
    if !silent {
        let msg = if is_internal {
            GAZE_MSG_FACE_VERIFIED
        } else {
            FACE_VERIFIED
        };
        unsafe { say(pamh, msg) };
    }
}

pub unsafe fn report_outcome(
    pamh: PamHandle,
    service: Option<&str>,
    silent: bool,
    text: &str,
    is_internal: bool,
) {
    if is_internal {
        if !silent {
            unsafe { report(pamh, service, text) };
        }
        return;
    }
    if silent {
        let _ = write_tty_line(service, text);
        return;
    }
    unsafe { report(pamh, service, text) }
}

fn confirm_from_tty(service: Option<&str>, prompt: PromptLine) -> Option<bool> {
    let mut tty = service_tty(service)?;
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
        if libc::tcsetattr(fd, libc::TCSAFLUSH, &raw) != 0 {
            return None;
        }

        let _guard = TermiosGuard { fd, original };
        replace_previous_line(&mut tty, prompt, CONFIRMATION_PROMPT).ok()?;
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
// isatty(STDIN_FILENO) misses a real controlling terminal whenever stdin is redirected, e.g.
// `echo 1 | sudo tee /tmp/1`; opening /dev/tty is how sudo itself finds the terminal.
fn open_interactive_tty() -> Option<std::fs::File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .ok()
}

pub fn has_interactive_tty() -> bool {
    open_interactive_tty().is_some()
}

/// Fallback confirmation without a controlling terminal.
/// Empty replies are not consent: some hosts return them without user input.
pub const TYPED_CONFIRMATION_PROMPT: &str = "Face Verified. Type 'yes' to confirm.";

pub fn typed_confirmation_accepted(response: Option<&str>) -> bool {
    response.is_some_and(|resp| resp.trim().eq_ignore_ascii_case("yes"))
}

pub unsafe fn confirm_authentication(
    pamh: PamHandle,
    service: Option<&str>,
    prompt: PromptLine,
) -> bool {
    if let Some(confirmed) = confirm_from_tty(service, prompt) {
        return confirmed;
    }

    unsafe { converse(pamh, PAM_PROMPT_ECHO_ON, TYPED_CONFIRMATION_PROMPT) }
        .is_some_and(|resp| typed_confirmation_accepted(Some(&resp)))
}

pub unsafe fn confirm_authentication_internal(pamh: PamHandle) -> bool {
    let resp = unsafe { converse(pamh, PAM_PROMPT_ECHO_ON, GAZE_REQUIRE_CONFIRMATION) }
        .or_else(|| unsafe { converse(pamh, PAM_PROMPT_ECHO_OFF, GAZE_REQUIRE_CONFIRMATION) });
    internal_prompt_confirmation_accepted(resp.as_ref().map(|s| s.as_str()))
}

pub fn confirmation_accepted(response: Option<&str>) -> bool {
    matches!(response, Some(""))
}

pub fn confirmation_required(
    auth: Option<&gaze_core::config::AuthConfig>,
    service: Option<&str>,
) -> bool {
    let surface = gaze_core::config::classify_pam_service(service);
    auth.is_none_or(|auth| auth.requires_confirmation(surface))
}

pub fn confirmation_requires_fallback(
    auth: Option<&gaze_core::config::AuthConfig>,
    service: Option<&str>,
) -> bool {
    confirmation_required(auth, service)
        && (service_cannot_be_prompted(service) || is_kde_login_greeter(service))
}

pub struct AuthState {
    pub password: Option<SecretBytes>,
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

/// Signals a host uses to cut its own password prompt short: Ctrl-C, sudo's timeout and the like.
pub fn host_prompt_signals() -> libc::sigset_t {
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::sigemptyset(&mut set);
        for sig in [
            libc::SIGINT,
            libc::SIGQUIT,
            libc::SIGTERM,
            libc::SIGHUP,
            libc::SIGALRM,
            libc::SIGTSTP,
        ] {
            libc::sigaddset(&mut set, sig);
        }
    }
    set
}

pub fn spawn_prompt_thread(
    pamh: PamHandle,
    state: &SharedAuthState,
    on_finished: impl FnOnce() + Send + 'static,
) -> thread::JoinHandle<()> {
    let thread_state = Arc::clone(state);
    let pamh_worker = pamh as usize;
    thread::spawn(move || {
        let host_signals = host_prompt_signals();
        unsafe { libc::pthread_sigmask(libc::SIG_UNBLOCK, &host_signals, ptr::null_mut()) };
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

pub fn wait_for_prompt_response(state: &SharedAuthState) -> Option<SecretBytes> {
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

pub unsafe fn stash_password_and_fallback(pamh: PamHandle, password: &[u8]) -> c_int {
    stash_password(password, |token| unsafe {
        pam_set_item(pamh, PAM_AUTHTOK, token.as_ptr().cast())
    })
}

fn stash_password(password: &[u8], set_token: impl FnOnce(&[u8]) -> c_int) -> c_int {
    if password.contains(&0) {
        return PAM_AUTH_ERR;
    }
    // Reserve the terminator up front, avoiding an unwiped allocation on reallocation.
    let mut token = SecretBytes::new(Vec::with_capacity(password.len() + 1));
    token.extend_from_slice(password);
    token.push(0);
    // Linux-PAM copies the token; our mutable, zeroizing buffer is wiped on every exit.
    if set_token(&token) == PAM_SUCCESS {
        PAM_AUTHINFO_UNAVAIL
    } else {
        PAM_AUTH_ERR
    }
}

pub fn give_up_message(status: Option<gaze_core::dbus::CaptureStatus>) -> &'static str {
    match status {
        Some(gaze_core::dbus::CaptureStatus::TooDark) => FACE_TOO_DARK,
        Some(gaze_core::dbus::CaptureStatus::NoFace) | None => FACE_NOT_DETECTED,
        Some(gaze_core::dbus::CaptureStatus::Unused) => FACE_UNAVAILABLE,
        _ => FACE_NOT_RECOGNIZED,
    }
}

// Confirm a face match through a graphical polkit dialog; the caller must
// already have a pending password prompt on `state` for the agent to answer.
pub unsafe fn confirm_graphical_polkit(
    pamh: PamHandle,
    state: &SharedAuthState,
    prompt_thread: thread::JoinHandle<()>,
    is_internal: bool,
) -> c_int {
    let prompt = if is_internal {
        GAZE_REQUIRE_CONFIRMATION
    } else {
        "Face Verified. Press Enter to confirm."
    };
    unsafe { say(pamh, prompt) };

    let response = wait_for_prompt_response(state);
    let _ = prompt_thread.join();

    let Some(resp) = response else {
        return PAM_AUTH_ERR;
    };
    let resp_str = std::str::from_utf8(&resp).ok();
    let confirmed = if is_internal {
        internal_confirmation_accepted(resp_str)
    } else {
        confirmation_accepted(resp_str)
    };
    if confirmed {
        PAM_SUCCESS
    } else {
        unsafe { stash_password_and_fallback(pamh, &resp) }
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

pub unsafe fn prompt_password(pamh: PamHandle) -> Option<SecretBytes> {
    unsafe { converse_bytes(pamh, PAM_PROMPT_ECHO_OFF, "Password: ") }
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

    // One thread per attempt: a worker per CPU inside sudo or a greeter buys nothing here.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| PAM_AUTHINFO_UNAVAIL)?;
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
        Ok(Some(mut config)) => {
            // The legacy Config property omits this flag.
            // VerifyStartForKeyring checks active prerequisites before authentication.
            let storage = Config::load().unwrap_or_default().storage;
            config.storage.unlock_gnome_keyring = storage.unlock_gnome_keyring;
            config.storage.unlock_kwallet = storage.unlock_kwallet;
            config.clamp_keyring();
            config
        }
        Ok(None) => {
            let mut config = Config::load_from(gaze_core::config::CONFIG_PATH).unwrap_or_default();
            // An incompatible daemon cannot support credential release.
            config.storage.unlock_gnome_keyring = false;
            config.storage.unlock_kwallet = false;
            config
        }
        Err(_) => return Err(PAM_SERVICE_ERR),
    };
    Ok((config, proxy))
}

pub async fn has_enrolled_faces_on(proxy: &GazeProxy<'_>, username: &str) -> anyhow::Result<bool> {
    match proxy.list_faces(username).await {
        // Treat unenrolled users as having no faces.
        Ok(faces) => Ok(faces.iter().any(|(_, templates, _, _)| *templates > 0)),
        Err(ref err) if gaze_core::dbus::dbus_is_file_not_found(err) => Ok(false),
        Err(err) => Err(err.into()),
    }
}

pub async fn has_enrolled_faces(username: &str) -> anyhow::Result<bool> {
    let (_config, proxy) = setup_auth_env()
        .await
        .map_err(|e| anyhow::anyhow!("PAM error: {e}"))?;
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

/// Uses the status embedded in the verification result, with the same priority as daemon `FaceStatus`.
/// The separate status stream may be stale, absent, or lose the race against verification in `select!`.
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

pub(crate) fn is_kwallet_login(service: Option<&str>) -> bool {
    matches!(
        service,
        Some("sddm" | "plasmalogin" | "plasmalogin-fingerprint")
    )
}

async fn request_verify_start(
    proxy: &GazeProxy<'static>,
    service: Option<&str>,
    require_keyring: bool,
) -> anyhow::Result<()> {
    if require_keyring {
        // No legacy fallback: older daemons cannot guarantee the active prerequisites.
        if let Some(service) = service.filter(|s| is_kwallet_login(Some(s))) {
            return proxy
                .verify_start_for_kwallet(service)
                .await
                .map_err(|e| anyhow::anyhow!("KWallet verification start failed: {e}"));
        }
        return proxy
            .verify_start_for_keyring()
            .await
            .map_err(|e| anyhow::anyhow!("Keyring verification start failed: {e}"));
    }
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
                .map_err(|e| anyhow::anyhow!("Verify start failed: {e}"))
        }
        other => other.map_err(|e| anyhow::anyhow!("Verify start failed: {e}")),
    }
}

pub async fn authenticate_biometric_with_status_on_and_notify<F>(
    proxy: &GazeProxy<'static>,
    username: &str,
    service: Option<&str>,
    require_keyring: bool,
    on_status: F,
) -> anyhow::Result<(AuthOutcome, Option<gaze_core::dbus::CaptureStatus>)>
where
    F: Fn(gaze_core::dbus::CaptureStatus),
{
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
        .map_err(|e| anyhow::anyhow!("Stream failed: {e}"))?;
    let mut face_stream = proxy
        .receive_face_status()
        .await
        .map_err(|e| anyhow::anyhow!("Stream failed: {e}"))?;
    request_verify_start(proxy, service, require_keyring).await?;

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
                    let status = *args.status();
                    last_status = Some(status);
                    on_status(status);
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

pub async fn authenticate_biometric_with_status_on(
    proxy: &GazeProxy<'static>,
    username: &str,
    service: Option<&str>,
    require_keyring: bool,
) -> anyhow::Result<(AuthOutcome, Option<gaze_core::dbus::CaptureStatus>)> {
    authenticate_biometric_with_status_on_and_notify(
        proxy,
        username,
        service,
        require_keyring,
        |_| {},
    )
    .await
}

pub unsafe fn get_pam_service(pamh: PamHandle) -> Option<String> {
    unsafe { get_pam_string(pamh, PAM_SERVICE) }
}

// Lossy rather than `None`: an undecodable host is still a remote one.
pub unsafe fn get_pam_rhost(pamh: PamHandle) -> Option<String> {
    unsafe { get_pam_bytes(pamh, PAM_RHOST) }
        .map(|host| String::from_utf8_lossy(&host).into_owned())
}

pub unsafe fn get_pam_tty(pamh: PamHandle) -> Option<String> {
    unsafe { get_pam_string(pamh, PAM_TTY) }
}

unsafe fn get_pam_bytes(pamh: PamHandle, item_type: c_int) -> Option<Vec<u8>> {
    let mut item_ptr: *const c_void = std::ptr::null();
    let ret = unsafe { pam_get_item(pamh, item_type, &mut item_ptr) };
    if ret != PAM_SUCCESS || item_ptr.is_null() {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(item_ptr as *const c_char) }
            .to_bytes()
            .to_vec(),
    )
}

unsafe fn get_pam_string(pamh: PamHandle, item_type: c_int) -> Option<String> {
    let mut item_ptr: *const c_void = std::ptr::null();
    let ret = unsafe { pam_get_item(pamh, item_type, &mut item_ptr) };
    if ret != PAM_SUCCESS || item_ptr.is_null() {
        return None;
    }
    unsafe {
        CStr::from_ptr(item_ptr as *const c_char)
            .to_str()
            .ok()
            .map(|s| s.to_owned())
    }
}

#[cfg(test)]
mod tests;
