// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::service::{
    OMARCHY_FINGERPRINT_SERVICE, OMARCHY_PASSWORD_SERVICE, face_slots_outranking,
};
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
fn only_terminal_services_use_the_terminal() {
    for service in ["sudo", "su", "login"] {
        assert!(service_reads_the_tty(Some(service)), "{service}");
    }
    for service in ["hyprlock-gaze", "kde", "sddm", "polkit-1"] {
        assert!(
            write_tty_line(Some(service), "unused").is_none(),
            "{service}"
        );
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

// The escape moves up a line and clears it, so it must only run when a line was printed.
#[test]
fn a_missing_prompt_line_is_not_cleared() {
    assert_eq!(line_prefix(PromptLine::Printed), "\x1B[1A\x1B[2K\r");
    assert_eq!(line_prefix(PromptLine::Absent), "\r");
}

#[test]
fn a_confirmation_prompt_is_still_written_without_a_prompt_line() {
    let mut out = Vec::new();
    replace_previous_line(&mut out, PromptLine::Absent, CONFIRMATION_PROMPT).unwrap();
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

// On timeout, decline instead of treating the terminal as absent and prompting again
// without a deadline.
#[test]
fn an_unanswered_prompt_declines_rather_than_reprompting() {
    assert!(!tty_confirmation(0, b'\n'));
    assert!(!tty_confirmation(0, 0));
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
fn kde_confirmation_falls_back_instead_of_unlocking_without_consent() {
    let on = gaze_core::config::AuthConfig {
        require_confirmation_lock_screen: true,
        require_confirmation_elevation: true,
        ..Default::default()
    };
    let off = gaze_core::config::AuthConfig::default();
    for service in [
        "kde-fingerprint",
        "kde-smartcard",
        "plasmalogin-fingerprint",
        "sddm",
        "plasmalogin",
    ] {
        assert!(confirmation_required(Some(&on), Some(service)));
        assert!(confirmation_required(None, Some(service)));
        assert!(confirmation_requires_fallback(Some(&on), Some(service)));
        assert!(confirmation_requires_fallback(None, Some(service)));
        assert!(!confirmation_requires_fallback(Some(&off), Some(service)));
    }
    for service in ["kde", "lightdm", "gdm-face", "sudo", "polkit-1"] {
        assert!(confirmation_required(Some(&on), Some(service)));
        assert!(!confirmation_requires_fallback(Some(&on), Some(service)));
    }
}

#[test]
fn stashed_passwords_preserve_bytes_and_add_only_the_terminator() {
    for password in [b"".as_slice(), b"synthetic-password", b"\xff\xfe"] {
        assert_eq!(
            stash_password(password, |token| {
                assert_eq!(&token[..password.len()], password);
                assert_eq!(token.last(), Some(&0));
                assert_eq!(token.len(), password.len() + 1);
                assert_ne!(token.as_ptr(), password.as_ptr());
                PAM_SUCCESS
            }),
            PAM_AUTHINFO_UNAVAIL
        );
    }
}

#[test]
fn a_nul_password_is_rejected_before_copying_or_setting_a_token() {
    for password in [b"\0".as_slice(), b"a\0b", b"a\0"] {
        assert_eq!(
            stash_password(password, |_| panic!("invalid token was handed to PAM")),
            PAM_AUTH_ERR
        );
    }
}

#[test]
fn a_failed_token_handoff_does_not_claim_password_fallback_is_ready() {
    assert_eq!(
        stash_password(b"synthetic-password", |_| PAM_SERVICE_ERR),
        PAM_AUTH_ERR
    );
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
fn confirmation_accepts_empty_string_on_enter() {
    assert!(confirmation_accepted(Some("")));
    assert!(!confirmation_accepted(Some("hunter2")));
    assert!(!confirmation_accepted(Some("confirm")));
    assert!(!confirmation_accepted(None));
}

#[test]
fn typed_confirmation_never_takes_an_empty_response() {
    // Do not treat an empty reply from an unknown prompt as confirmation.
    assert!(!typed_confirmation_accepted(None));
    assert!(!typed_confirmation_accepted(Some("")));
    assert!(!typed_confirmation_accepted(Some("   ")));
    assert!(!typed_confirmation_accepted(Some("\n")));

    assert!(typed_confirmation_accepted(Some("yes")));
    assert!(typed_confirmation_accepted(Some("YES")));
    assert!(typed_confirmation_accepted(Some("  yes\n")));

    assert!(!typed_confirmation_accepted(Some("y")));
    assert!(!typed_confirmation_accepted(Some("confirm")));
    assert!(!typed_confirmation_accepted(Some("hunter2")));
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

    replace_previous_line(&mut output, PromptLine::Printed, FACE_VERIFIED).unwrap();

    assert_eq!(
        String::from_utf8(output).unwrap(),
        "\x1B[1A\x1B[2K\rFace Verified."
    );
    assert!(!FACE_VERIFIED.contains("confirm"));
}

#[test]
fn a_silenced_conversation_still_leaves_a_terminal_verdict() {
    let mut output = Vec::new();

    replace_previous_line(&mut output, PromptLine::Absent, FACE_VERIFIED).unwrap();

    assert_eq!(String::from_utf8(output).unwrap(), "\rFace Verified.");
}

#[test]
fn a_silenced_prompt_reported_on_the_tty_still_owns_a_line() {
    assert!(!PromptLine::Printed.keeps_previous_line());
    assert!(PromptLine::Absent.keeps_previous_line());
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
fn only_credential_setup_after_authentication_clears_duress() {
    assert!(setcred_follows_authentication(PAM_ESTABLISH_CRED));
    assert!(setcred_follows_authentication(PAM_REINITIALIZE_CRED));
    assert!(setcred_follows_authentication(
        PAM_REFRESH_CRED | PAM_SILENT
    ));
    assert!(!setcred_follows_authentication(PAM_DELETE_CRED));
    assert!(!setcred_follows_authentication(
        PAM_DELETE_CRED | PAM_ESTABLISH_CRED
    ));
    assert!(!setcred_follows_authentication(0));
}

#[test]
fn only_a_successful_pam_end_clears_duress() {
    assert!(pam_end_reports_success(PAM_SUCCESS));
    assert!(pam_end_reports_success(PAM_SUCCESS | PAM_DATA_SILENT));
    assert!(!pam_end_reports_success(PAM_AUTH_ERR));
    assert!(!pam_end_reports_success(PAM_AUTH_ERR | PAM_DATA_SILENT));
    assert!(!pam_end_reports_success(PAM_SUCCESS | PAM_DATA_REPLACE));
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

#[test]
fn pam_internal_service_matching_checks_normalized_names() {
    let internal_services = vec!["polkit-1".to_string(), "gdm-face".to_string()];

    assert!(is_service_internal("polkit-1", &internal_services));
    assert!(is_service_internal("gdm-face", &internal_services));
    assert!(is_service_internal(
        "/etc/pam.d/polkit-1",
        &internal_services
    ));
    assert!(is_service_internal(
        "/etc/pam.d/gdm-face",
        &internal_services
    ));
    assert!(is_service_internal("  polkit-1  ", &internal_services));

    assert!(!is_service_internal("sudo", &internal_services));
    assert!(!is_service_internal("hyprlock", &internal_services));
    assert!(!is_service_internal("gdm-password", &internal_services));
    assert!(!is_service_internal("", &internal_services));

    let empty: Vec<String> = Vec::new();
    assert!(!is_service_internal("polkit-1", &empty));
}

#[test]
fn pam_internal_give_up_messages_map_correctly() {
    use gaze_core::dbus::{
        CaptureStatus, GAZE_MSG_FACE_NOT_DETECTED, GAZE_MSG_FACE_NOT_RECOGNIZED,
        GAZE_MSG_FACE_TOO_DARK, GAZE_MSG_FACE_UNAVAILABLE,
    };

    assert_eq!(
        internal_give_up_message(Some(CaptureStatus::Usable)),
        GAZE_MSG_FACE_NOT_RECOGNIZED
    );
    assert_eq!(
        internal_give_up_message(Some(CaptureStatus::NoFace)),
        GAZE_MSG_FACE_NOT_DETECTED
    );
    assert_eq!(
        internal_give_up_message(Some(CaptureStatus::TooDark)),
        GAZE_MSG_FACE_TOO_DARK
    );
    assert_eq!(
        internal_give_up_message(Some(CaptureStatus::Unused)),
        GAZE_MSG_FACE_UNAVAILABLE
    );
    assert_eq!(
        internal_give_up_message(Some(CaptureStatus::Clipped)),
        GAZE_MSG_FACE_NOT_RECOGNIZED
    );
    assert_eq!(internal_give_up_message(None), GAZE_MSG_FACE_NOT_DETECTED);
}

#[test]
fn pam_internal_confirmation_strictly_requires_gaze_confirmed() {
    use gaze_core::dbus::GAZE_CONFIRMED;

    assert!(internal_confirmation_accepted(Some(GAZE_CONFIRMED)));
    assert!(internal_confirmation_accepted(Some("  GAZE_CONFIRMED\n")));
    assert!(internal_confirmation_accepted(Some("GAZE_CONFIRMED\r\n")));

    // A bare ENTER confirms in standard mode, so internal mode must not accept it.
    assert!(!internal_confirmation_accepted(Some("")));
    assert!(!internal_confirmation_accepted(Some("\n")));
    assert!(!internal_confirmation_accepted(Some("   ")));
    assert!(!internal_confirmation_accepted(None));

    assert!(!internal_confirmation_accepted(Some("yes")));
    assert!(!internal_confirmation_accepted(Some("GAZE_CANCEL")));
    assert!(!internal_confirmation_accepted(Some("gaze_confirmed")));
}

#[test]
fn a_plain_internal_prompt_still_takes_a_bare_newline() {
    use gaze_core::dbus::GAZE_CONFIRMED;

    assert!(internal_prompt_confirmation_accepted(Some(GAZE_CONFIRMED)));
    assert!(internal_prompt_confirmation_accepted(Some("")));
    assert!(internal_prompt_confirmation_accepted(Some("\n")));
    assert!(internal_prompt_confirmation_accepted(Some("   ")));

    assert!(!internal_prompt_confirmation_accepted(None));
    assert!(!internal_prompt_confirmation_accepted(Some("yes")));
    assert!(!internal_prompt_confirmation_accepted(Some("GAZE_CANCEL")));
}

#[test]
fn verdict_survives_the_round_trip_through_pam_data() {
    for verdict in [
        FirstPassVerdict::NoMatch,
        FirstPassVerdict::Undecided,
        FirstPassVerdict::Preempted,
    ] {
        assert_eq!(FirstPassVerdict::from_repr(verdict as u8), Some(verdict));
    }
}

#[test]
fn an_unknown_verdict_byte_is_rejected() {
    assert_eq!(FirstPassVerdict::from_repr(0), None);
    assert_eq!(FirstPassVerdict::from_repr(4), None);
    assert_eq!(FirstPassVerdict::from_repr(255), None);
}

#[test]
fn only_a_non_match_blocks_a_retry() {
    assert!(!FirstPassVerdict::NoMatch.allows_retry());
    assert!(FirstPassVerdict::Undecided.allows_retry());
    assert!(FirstPassVerdict::Preempted.allows_retry());
}
