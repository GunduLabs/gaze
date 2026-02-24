use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

use gaze_common::camera::Camera;
use gaze_common::config::Config;
use gaze_common::dbus::AuthProxyBlocking;
use opencv::prelude::*;
use zbus::blocking::Connection;

const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_SERVICE_ERR: c_int = 3;

type PamHandle = *mut libc::c_void;

unsafe extern "C" {
    fn pam_get_user(pamh: PamHandle, user: *mut *const c_char, prompt: *const c_char) -> c_int;
}

fn capture_frame_bytes(config: &Config) -> Option<(Vec<u8>, u32, u32)> {
    let mut cam = Camera::open(&config.cameras.rgb).ok()?;
    let frame = cam.capture_frame().ok()?;
    let sz = frame.size().ok()?;
    let total_bytes = (sz.width * sz.height * 3) as usize;
    let mut bytes = vec![0u8; total_bytes];
    unsafe {
        std::ptr::copy_nonoverlapping(frame.data(), bytes.as_mut_ptr(), total_bytes);
    }
    Some((bytes, sz.width as u32, sz.height as u32))
}

fn do_authenticate(pamh: PamHandle) -> c_int {
    let username = unsafe {
        let mut user_ptr: *const c_char = std::ptr::null();
        let ret = pam_get_user(pamh, &mut user_ptr, std::ptr::null());
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

    let (bytes, width, height) = match capture_frame_bytes(&config) {
        Some(f) => f,
        None => return PAM_SERVICE_ERR,
    };

    let conn = match Connection::system() {
        Ok(c) => c,
        Err(_) => return PAM_SERVICE_ERR,
    };

    let proxy = match AuthProxyBlocking::new(&conn) {
        Ok(p) => p,
        Err(_) => return PAM_SERVICE_ERR,
    };

    match proxy.authenticate(&username, &bytes, width, height) {
        Ok(face) if !face.is_empty() => PAM_SUCCESS,
        Ok(_) => PAM_AUTH_ERR,
        Err(_) => PAM_SERVICE_ERR,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pam_sm_authenticate(
    pamh: PamHandle,
    _flags: c_int,
    _argc: c_int,
    _argv: *const *const c_char,
) -> c_int {
    do_authenticate(pamh)
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
