use std::ffi::CStr;
use std::os::raw::{c_char, c_int};

use zbus::blocking::Connection;
use zbus::proxy;

const PAM_SUCCESS: c_int = 0;
const PAM_AUTH_ERR: c_int = 7;
const PAM_SERVICE_ERR: c_int = 3;

type PamHandle = *mut libc::c_void;

unsafe extern "C" {
    fn pam_get_user(pamh: PamHandle, user: *mut *const c_char, prompt: *const c_char) -> c_int;
}

#[proxy(
    interface = "org.gaze.Auth",
    default_service = "org.gaze.Auth",
    default_path = "/org/gaze/Auth"
)]
trait Auth {
    fn authenticate(&self, username: &str) -> zbus::Result<bool>;
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

    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return PAM_SERVICE_ERR,
    };

    rt.block_on(async {
        let conn = match Connection::system() {
            Ok(c) => c,
            Err(_) => return PAM_SERVICE_ERR,
        };

        let proxy = match AuthProxyBlocking::new(&conn) {
            Ok(p) => p,
            Err(_) => return PAM_SERVICE_ERR,
        };

        match proxy.authenticate(&username) {
            Ok(true) => PAM_SUCCESS,
            Ok(false) => PAM_AUTH_ERR,
            Err(_) => PAM_SERVICE_ERR,
        }
    })
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
