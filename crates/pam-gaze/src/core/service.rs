// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::{
    FACE_PAM_SERVICE, KDE_FACE_PAM_FILE, KDE_SMARTCARD_PAM_FILE, PLASMALOGIN_FACE_PAM_FILE,
    pam_service_name,
};

pub fn caller_is_remote(rhost: Option<&str>) -> bool {
    match rhost {
        None => false,
        Some(host) => {
            let host = host.trim();
            !matches!(
                host,
                "" | "localhost" | "localhost.localdomain" | "127.0.0.1" | "::1"
            )
        }
    }
}

const NETWORK_PAM_SERVICES: [&str; 20] = [
    "sshd",
    "dovecot",
    "imap",
    "imaps",
    "pop3",
    "pop3s",
    "smtp",
    "sieve",
    "managesieve",
    "vsftpd",
    "proftpd",
    "pure-ftpd",
    "ftp",
    "samba",
    "cups",
    "openvpn",
    "radiusd",
    "ppp",
    "xrdp-sesman",
    "cockpit",
];

pub fn service_is_network_facing(service: Option<&str>) -> bool {
    service.is_some_and(|s| NETWORK_PAM_SERVICES.contains(&pam_service_name(s)))
}

pub fn face_auth_out_of_scope(service: Option<&str>, rhost: Option<&str>) -> bool {
    caller_is_remote(rhost) || service_is_network_facing(service)
}

/// Local services allowed to face-authenticate from a root PAM host.
/// Unknown services may serve network clients without PAM_RHOST and must use passwords.
const LOCAL_ROOT_SERVICES: [&str; 26] = [
    "sudo",
    "sudo-i",
    "su",
    "su-l",
    "doas",
    "run0",
    "systemd-run0",
    "polkit-1",
    "pkexec",
    "login",
    "sddm",
    "lightdm",
    "greetd",
    "gdm-password",
    "gdm-launch-environment",
    FACE_PAM_SERVICE,
    KDE_FACE_PAM_SERVICE,
    KDE_SMARTCARD_PAM_SERVICE,
    "kde",
    "plasmalogin",
    PLASMALOGIN_FACE_PAM_SERVICE,
    OMARCHY_FACE_PAM_SERVICE,
    "cosmic-greeter",
    "ly",
    "lemurs",
    "lxdm",
];

/// `login` is also the default service of web logins such as JupyterHub, so it only counts as
/// local when it runs on a virtual console.
fn is_virtual_console(tty: Option<&str>) -> bool {
    tty.map(|tty| tty.trim_start_matches("/dev/"))
        .and_then(|tty| tty.strip_prefix("tty"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// Whether a root PAM host may face-authenticate this service. Unprivileged hosts (screen
/// lockers) can only reach the daemon for their own user, so this applies to root alone.
pub fn root_service_is_local(service: Option<&str>, tty: Option<&str>, trusted: bool) -> bool {
    if trusted {
        return true;
    }
    match service.map(pam_service_name) {
        Some("login") => is_virtual_console(tty),
        Some(name) => LOCAL_ROOT_SERVICES.contains(&name),
        None => false,
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

pub const KDE_SMARTCARD_PAM_SERVICE: &str = "kde-smartcard";

/// Plasma Login Manager's biometric helper, which runs alongside the password field instead
/// of after it. Gaze is wired into it only on distros that ship the service.
pub const PLASMALOGIN_FACE_PAM_SERVICE: &str = "plasmalogin-fingerprint";

pub const OMARCHY_FACE_PAM_SERVICE: &str = "gaze-omarchy-face";

pub const OMARCHY_FACE_PAM_FILE: &str = "/etc/pam.d/gaze-omarchy-face";

/// The interactive services driving the password field, which reach Gaze through
/// the shared stack it installs into.
const KDE_INTERACTIVE_SERVICE: &str = "kde";

const PLASMALOGIN_INTERACTIVE_SERVICE: &str = "plasmalogin";

const SDDM_INTERACTIVE_SERVICE: &str = "sddm";

pub(super) const OMARCHY_PASSWORD_SERVICE: &str = "omarchy-lock-password";

pub(super) const OMARCHY_FINGERPRINT_SERVICE: &str = "omarchy-lock-fingerprint";

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

pub(super) fn is_kde_login_greeter(service: Option<&str>) -> bool {
    matches!(
        service,
        Some(SDDM_INTERACTIVE_SERVICE | PLASMALOGIN_INTERACTIVE_SERVICE)
    )
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
        // NixOS writes an absolute store path, not a bare module name.
        field
            .rsplit('/')
            .next()
            .is_some_and(|name| GAZE_PAM_MODULES.contains(&name))
    })
}

fn pam_service_contents(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Which up-front slots a service must stand down for, so one unlock claims the camera once.
/// Plasma runs several PAM services per unlock, and the password-side ones also reach Gaze.
pub(super) fn face_slots_outranking(service: Option<&str>) -> &'static [&'static str] {
    match service {
        Some(KDE_INTERACTIVE_SERVICE) => &[KDE_FACE_PAM_FILE, KDE_SMARTCARD_PAM_FILE],
        Some(KDE_SMARTCARD_PAM_SERVICE) => &[KDE_FACE_PAM_FILE],
        Some(PLASMALOGIN_INTERACTIVE_SERVICE) => &[PLASMALOGIN_FACE_PAM_FILE],
        Some(OMARCHY_PASSWORD_SERVICE | OMARCHY_FINGERPRINT_SERVICE) => &[OMARCHY_FACE_PAM_FILE],
        _ => &[],
    }
}

pub fn service_defers_to_face_slot(service: Option<&str>) -> bool {
    face_slots_outranking(service)
        .iter()
        .any(|path| pam_stack_runs_gaze(pam_service_contents(path).as_deref()))
}

/// The greeter cannot route a response to this slot, so prompting would block it
/// for the rest of the lock screen session.
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
