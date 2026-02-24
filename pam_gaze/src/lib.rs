use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;

use gaze_common::camera::Camera;
use gaze_common::capture::frame_to_bytes;
use gaze_common::config::Config;
use gaze_common::dbus::AuthProxyBlocking;
use zbus::blocking::Connection;

const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_SERVICE_ERR: c_int = 3;
const PAM_CONV: c_int = 5;
const PAM_TEXT_INFO: c_int = 4;

const MAX_ATTEMPTS: usize = 10;

type PamHandle = *mut c_void;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: Option<
        unsafe extern "C" fn(
            num_msg: c_int,
            msg: *mut *const PamMessage,
            resp: *mut *mut PamResponse,
            appdata_ptr: *mut c_void,
        ) -> c_int,
    >,
    appdata_ptr: *mut c_void,
}

unsafe extern "C" {
    fn pam_get_user(pamh: PamHandle, user: *mut *const c_char, prompt: *const c_char) -> c_int;
    fn pam_get_item(pamh: PamHandle, item_type: c_int, item: *mut *const c_void) -> c_int;
}

fn say(pamh: PamHandle, text: &str) {
    let mut item: *const c_void = ptr::null();
    unsafe {
        if pam_get_item(pamh, PAM_CONV, &mut item) != PAM_SUCCESS || item.is_null() {
            return;
        }
        let conv = &*(item as *const PamConv);
        let Some(conv_fn) = conv.conv else { return };

        let msg_str = CString::new(text).unwrap();
        let msg = PamMessage {
            msg_style: PAM_TEXT_INFO,
            msg: msg_str.as_ptr(),
        };
        let mut msg_ptr = &msg as *const PamMessage;
        let mut resp_ptr: *mut PamResponse = ptr::null_mut();

        (conv_fn)(1, &mut msg_ptr, &mut resp_ptr, conv.appdata_ptr);

        if !resp_ptr.is_null() {
            if !(*resp_ptr).resp.is_null() {
                let _ = CString::from_raw((*resp_ptr).resp);
            }
            libc::free(resp_ptr as *mut c_void);
        }
    }
}

fn is_retryable(err: &zbus::Error) -> bool {
    err.to_string().contains("RETRYABLE:")
}

fn do_authenticate(pamh: PamHandle) -> c_int {
    let username = unsafe {
        let mut user_ptr: *const c_char = ptr::null();
        let ret = pam_get_user(pamh, &mut user_ptr, ptr::null());
        if ret != PAM_SUCCESS || user_ptr.is_null() {
            return PAM_AUTH_ERR;
        }
        match CStr::from_ptr(user_ptr).to_str() {
            Ok(s) => s.to_owned(),
            Err(_) => return PAM_AUTH_ERR,
        }
    };

    let config = match Config::load() {
        Ok(c) => c,
        Err(_) => return PAM_SERVICE_ERR,
    };

    let mut cam = match Camera::open(&config.cameras.rgb) {
        Ok(c) => c,
        Err(_) => return PAM_SERVICE_ERR,
    };

    let conn = match Connection::system() {
        Ok(c) => c,
        Err(_) => return PAM_SERVICE_ERR,
    };
    let proxy = match AuthProxyBlocking::new(&conn) {
        Ok(p) => p,
        Err(_) => return PAM_SERVICE_ERR,
    };

    say(pamh, "Please look at the camera");

    for _ in 0..MAX_ATTEMPTS {
        let frame = match cam.capture_frame() {
            Ok(f) => f,
            Err(_) => continue,
        };
        let capture = match frame_to_bytes(&frame) {
            Ok(c) => c,
            Err(_) => continue,
        };

        match proxy.authenticate(&username, &capture.bytes, capture.width, capture.height) {
            Ok(face) if !face.is_empty() => {
                return PAM_SUCCESS;
            }
            Ok(_) => {
                return PAM_AUTH_ERR;
            }
            Err(ref err) if is_retryable(err) => continue,
            Err(_) => {
                return PAM_SERVICE_ERR;
            }
        }
    }

    PAM_AUTH_ERR
}

/// # Safety
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    do_authenticate(pamh)
}

/// # Safety
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_setcred(
    _pamh: PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    PAM_SUCCESS
}

/// # Safety
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_acct_mgmt(
    _pamh: PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    PAM_SUCCESS
}
