#![allow(clippy::missing_safety_doc)]
use std::ffi::{CStr, CString};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, RawFd};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

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

pub type PamHandle = *mut c_void;

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

/// True if the process has a controlling terminal we can prompt on.
pub fn has_controlling_tty() -> bool {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
}

/// Prompt for a password on the controlling terminal, cancellable via `cancel_fd`.
///
/// Reads one line from `/dev/tty` with echo disabled (kernel line-editing stays
/// on). It `poll`s the tty alongside `cancel_fd`; when the caller makes
/// `cancel_fd` readable (after biometric auth wins the race) the prompt is
/// abandoned, any half-typed input is flushed, and `None` is returned. This lets
/// the caller retire the prompt thread cleanly without TIOCSTI, which modern
/// kernels disable (`dev.tty.legacy_tiocsti=0`).
pub fn prompt_password_from_tty(cancel_fd: RawFd) -> Option<String> {
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
        raw.c_lflag &= !libc::ECHO; // hide the password; keep ICANON for line editing
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return None;
        }

        let _guard = TermiosGuard { fd, original };
        write!(tty, "Password: ").ok()?;
        tty.flush().ok()?;

        let mut poll_fds = [
            libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: cancel_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];

        loop {
            if libc::poll(poll_fds.as_mut_ptr(), 2, -1) < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return None;
            }

            // Cancelled (biometric won): drop any half-typed line and give up.
            if poll_fds[1].revents != 0 {
                let _ = libc::tcflush(fd, libc::TCIFLUSH);
                let _ = writeln!(tty);
                return None;
            }

            if poll_fds[0].revents != 0 {
                let mut buf = [0_u8; 1024];
                let n = libc::read(fd, buf.as_mut_ptr() as *mut c_void, buf.len());
                let _ = writeln!(tty);
                if n <= 0 {
                    return None;
                }
                let mut pw = String::from_utf8_lossy(&buf[..n as usize]).into_owned();
                while pw.ends_with('\n') || pw.ends_with('\r') {
                    pw.pop();
                }
                return Some(pw);
            }
        }
    }
}

pub unsafe fn confirm_authentication(pamh: PamHandle) -> bool {
    if let Some(confirmed) = confirm_from_tty() {
        return confirmed;
    }

    unsafe { converse(pamh, PAM_PROMPT_ECHO_ON, CONFIRMATION_PROMPT) }
        .map(|resp| resp.is_empty())
        .unwrap_or(false)
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

pub fn is_retryable(err: &zbus::Error) -> bool {
    err.to_string().contains("RETRYABLE:")
}

use gaze_core::dbus::GazeProxy;
pub use zbus::Connection;

pub async fn setup_auth_env() -> Result<(Config, GazeProxy<'static>), c_int> {
    let conn = Connection::system().await.map_err(|_| PAM_SERVICE_ERR)?;
    let proxy = GazeProxy::new(&conn).await.map_err(|_| PAM_SERVICE_ERR)?;
    let config = gaze_core::dbus::load_config_from_daemon(&proxy)
        .await
        .map_err(|_| PAM_SERVICE_ERR)?;
    Ok((config, proxy))
}

pub async fn has_enrolled_faces(username: &str) -> anyhow::Result<bool> {
    let (_config, proxy) = setup_auth_env()
        .await
        .map_err(|e| anyhow::anyhow!("PAM error: {}", e))?;
    let faces = proxy.list_faces(username).await?;
    Ok(!faces.is_empty())
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

pub async fn authenticate_biometric(username: &str) -> anyhow::Result<Option<bool>> {
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

    let mut status_stream = proxy
        .receive_verify_status()
        .await
        .map_err(|e| anyhow::anyhow!("Stream failed: {}", e))?;
    proxy
        .verify_start("any")
        .await
        .map_err(|e| anyhow::anyhow!("Verify start failed: {}", e))?;

    let mut matched = false;
    use futures::StreamExt;
    while let Some(signal) = status_stream.next().await {
        if let Ok(args) = signal.args() {
            match *args.result() {
                gaze_core::dbus::VerifyResult::VerifyMatch => {
                    matched = true;
                    break;
                }
                gaze_core::dbus::VerifyResult::VerifyNoMatch => break,
            }
        }
    }

    guard.active = false;
    let _ = proxy.release().await;
    Ok(Some(matched))
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

pub fn get_user_name_by_uid(uid: u32) -> Option<String> {
    unsafe {
        let pwd = libc::getpwuid(uid);
        if !pwd.is_null() {
            Some(
                CStr::from_ptr((*pwd).pw_name)
                    .to_string_lossy()
                    .into_owned(),
            )
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

pub fn is_text_environment() -> bool {
    let is_tty = unsafe { libc::isatty(0) == 1 };
    is_tty
        && std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
