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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_logins_are_not_face_authenticated() {
        for rhost in [
            "192.168.1.120",
            "mail.example.com",
            "2001:db8::1",
            "10.0.0.4",
        ] {
            assert!(caller_is_remote(Some(rhost)), "{rhost}");
        }
    }

    #[test]
    fn local_callers_are_face_authenticated() {
        for rhost in [
            None,
            Some(""),
            Some("  "),
            Some("localhost"),
            Some("localhost.localdomain"),
            Some("127.0.0.1"),
            Some("::1"),
        ] {
            assert!(!caller_is_remote(rhost), "{rhost:?}");
        }
    }

    #[test]
    fn root_hosts_only_face_authenticate_known_local_services() {
        for service in ["sudo", "polkit-1", "gdm-password", "sddm", "/etc/pam.d/su"] {
            assert!(
                root_service_is_local(Some(service), None, false),
                "{service}"
            );
        }
        for service in ["jupyterhub", "webmin", "rstudio", "vnc", ""] {
            assert!(
                !root_service_is_local(Some(service), None, false),
                "{service}"
            );
        }
        assert!(!root_service_is_local(None, None, false));
        assert!(root_service_is_local(Some("webmin"), None, true));
    }

    #[test]
    fn login_is_local_only_on_a_virtual_console() {
        for tty in ["tty1", "/dev/tty3", "tty12"] {
            assert!(
                root_service_is_local(Some("login"), Some(tty), false),
                "{tty}"
            );
        }
        for tty in [
            None,
            Some(""),
            Some("tty"),
            Some("ttyS0"),
            Some("pts/0"),
            Some(":0"),
        ] {
            assert!(!root_service_is_local(Some("login"), tty, false), "{tty:?}");
        }
    }

    #[test]
    fn network_services_are_skipped_even_over_loopback() {
        for service in ["dovecot", "imap", "sshd", "vsftpd", "/etc/pam.d/dovecot"] {
            assert!(service_is_network_facing(Some(service)), "{service}");
            assert!(face_auth_out_of_scope(Some(service), Some("127.0.0.1")));
            assert!(face_auth_out_of_scope(Some(service), None));
        }
    }

    #[test]
    fn desktop_services_stay_in_scope_locally() {
        for service in [
            Some("sudo"),
            Some("polkit-1"),
            Some("gdm-face"),
            Some("login"),
            Some("kde"),
            None,
        ] {
            assert!(!service_is_network_facing(service), "{service:?}");
            assert!(!face_auth_out_of_scope(service, Some("localhost")));
        }
        assert!(face_auth_out_of_scope(Some("sudo"), Some("192.168.1.120")));
    }

    #[test]
    fn face_slots_are_outranked_in_one_direction_only() {
        for service in [OMARCHY_PASSWORD_SERVICE, OMARCHY_FINGERPRINT_SERVICE] {
            assert_eq!(
                face_slots_outranking(Some(service)),
                [OMARCHY_FACE_PAM_FILE]
            );
        }
        assert!(face_slots_outranking(Some(OMARCHY_FACE_PAM_SERVICE)).is_empty());
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
}
