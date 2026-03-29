#![allow(clippy::missing_safety_doc)]
use std::ffi::{CStr, CString};
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
pub const PAM_AUTHINFO_UNAVAIL: c_int = 9;

pub const MAX_ATTEMPTS: usize = 10;
pub const CAMERA_AUTH_TIMEOUT_SECS: u64 = 12;

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

unsafe fn converse(pamh: PamHandle, msg_style: c_int, text: &str) -> Option<String> {
    unsafe {
        let mut item: *const c_void = ptr::null();
        if pam_get_item(pamh, PAM_CONV, &mut item) != PAM_SUCCESS || item.is_null() {
            return None;
        }
        let conv = &*(item as *const PamConv);
        let conv_fn = conv.conv?;

        let msg_str = CString::new(text).unwrap();
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
                let _ = CString::from_raw(resp);
            }
            libc::free(resp_ptr as *mut c_void);
        }
        result
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

pub fn is_retryable(err: &zbus::Error) -> bool {
    err.to_string().contains("RETRYABLE:")
}

use gaze_core::dbus::GazeProxy;
pub use zbus::Connection;

pub async fn setup_auth_env() -> Result<(Config, GazeProxy<'static>), c_int> {
    let config = Config::load().map_err(|_| PAM_SERVICE_ERR)?;
    let conn = Connection::system().await.map_err(|_| PAM_SERVICE_ERR)?;
    let proxy = GazeProxy::new(&conn).await.map_err(|_| PAM_SERVICE_ERR)?;
    Ok((config, proxy))
}

pub async fn authenticate_biometric(username: &str) -> anyhow::Result<bool> {
    let (_config, proxy) = setup_auth_env()
        .await
        .map_err(|e| anyhow::anyhow!("PAM error: {}", e))?;
    proxy
        .claim(username)
        .await
        .map_err(|e| anyhow::anyhow!("Claim failed: {:?}", e))?;

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
        if let Ok(args) = signal.args()
            && *args.result() == gaze_core::dbus::VerifyResult::VerifyMatch
        {
            matched = true;
            break;
        }
    }

    let _ = proxy.release().await;
    Ok(matched)
}
