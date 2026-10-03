// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) fn check_tpm(report: &mut Report, config: Option<&Config>) {
    let Some(config) = config else {
        return;
    };
    if !config.storage.encrypt_templates {
        report.off(
            "TPM",
            "template encryption is off, so face templates sit on disk unencrypted and no TPM is required",
            format!("Turn it on: set `encrypt_templates = true` under [storage] in {CONFIG_PATH}, then restart gazed."),
        );
        return;
    }

    let present: Vec<&str> = TPM_DEVICES
        .iter()
        .copied()
        .filter(|path| Path::new(path).exists())
        .collect();

    if present.is_empty() {
        report.error(
            "TPM",
            "template encryption is enabled but no TPM device is present",
            "Enable TPM 2.0 in firmware or set storage.encrypt_templates = false, then restart gazed.",
        );
        return;
    }

    let Some(credentials) = daemon_credentials() else {
        report.pass("TPM", "a TPM device is present for encrypted templates");
        return;
    };

    let mut blocked = Vec::new();
    for path in &present {
        let Ok(meta) = fs::metadata(path) else {
            report.pass("TPM", "a TPM device is present for encrypted templates");
            return;
        };
        if node_openable(meta.uid(), meta.gid(), meta.mode(), &credentials) {
            report.pass(
                "TPM",
                format!("a TPM device is present and gazed can open {path}"),
            );
            return;
        }
        blocked.push(format!(
            "{path} is {}:{} {:04o}",
            user_name(meta.uid()),
            group_name(meta.gid()),
            meta.mode() & 0o777
        ));
    }

    report.error(
        "TPM",
        format!(
            "template encryption is enabled but the gazed unit cannot open the TPM device ({})",
            blocked.join(", ")
        ),
        "Run `sudo systemctl edit gazed` and add `SupplementaryGroups=tss` (or \
         `CapabilityBoundingSet=CAP_DAC_READ_SEARCH CAP_DAC_OVERRIDE`) under [Service], then run \
         `sudo systemctl restart gazed`.",
    );
}

pub(super) fn pam_entry(line: &str) -> Option<(&str, &str, &str, &str)> {
    let line = line.split('#').next()?.trim();
    let (kind, rest) = line.split_once(char::is_whitespace)?;
    let rest = rest.trim_start();
    // @include also affects auth; ignoring it would miscount pam_gaze's success=1 jump.
    if kind == "@include" {
        return Some(("auth", "include", rest, ""));
    }
    let (control, rest) = if rest.starts_with('[') {
        rest.split_at(rest.find(']')? + 1)
    } else {
        rest.split_once(char::is_whitespace)?
    };
    let (module, options) = rest
        .trim_start()
        .split_once(char::is_whitespace)
        .unwrap_or((rest.trim_start(), ""));
    let module = module.rsplit('/').next()?;
    Some((kind.trim_start_matches('-'), control, module, options))
}

/// Recognize the packaged hand-off, including the session hook that starts the keyring.
pub(super) fn gdm_face_stack_passes_the_token(contents: &str) -> bool {
    let entries: Vec<_> = contents.lines().filter_map(pam_entry).collect();
    let auth: Vec<_> = entries.iter().filter(|entry| entry.0 == "auth").collect();
    let handoff = auth.windows(3).any(|lines| {
        let (_, control, module, options) = *lines[0];
        module == "pam_gaze.so"
            && control
                .split_ascii_whitespace()
                .eq(["[success=1", "default=ignore]"])
            && !options
                .split_ascii_whitespace()
                .any(|option| option == "simultaneous")
            && lines[1].1 == "requisite"
            && lines[1].2 == "pam_deny.so"
            && lines[2].1 == "optional"
            && lines[2].2 == "pam_gnome_keyring.so"
            && lines[2]
                .3
                .split_ascii_whitespace()
                .any(|option| option == "use_authtok")
            && !lines[2]
                .3
                .split_ascii_whitespace()
                .any(|option| option == "auto_start" || option.starts_with("only_if="))
    });
    handoff && starts_keyring_session(&entries)
}

pub(super) fn starts_keyring_session(entries: &[(&str, &str, &str, &str)]) -> bool {
    entries.iter().any(|&(kind, control, module, options)| {
        kind == "session"
            && matches!(control, "optional" | "required")
            && module == "pam_gnome_keyring.so"
            && options
                .split_ascii_whitespace()
                .any(|option| option == "auto_start")
            && !options
                .split_ascii_whitespace()
                .any(|option| option.starts_with("only_if="))
    })
}

/// greetd has no token-only service like `gdm-face`, so the keyring line just has to come after
/// whatever runs `pam_gaze.so` without a face match jumping over it. A match then hands it the
/// released credential and a typed password hands it the one `pam_unix` read.
pub(super) fn greetd_stack_passes_the_token(contents: &str) -> bool {
    let entries: Vec<_> = contents.lines().filter_map(pam_entry).collect();
    let auth: Vec<_> = entries.iter().filter(|entry| entry.0 == "auth").collect();
    let Some(keyring) = auth.iter().position(|entry| {
        entry.1 == "optional"
            && entry.2 == "pam_gnome_keyring.so"
            && entry
                .3
                .split_ascii_whitespace()
                .any(|option| option == "use_authtok")
    }) else {
        return false;
    };
    let before = &auth[..keyring];
    let reaches_gaze = before
        .iter()
        .any(|entry| entry.2 == "pam_gaze.so" || matches!(entry.1, "substack" | "include"));
    let skips_keyring = before.iter().enumerate().any(|(index, entry)| {
        entry.2 == "pam_gaze.so"
            && success_skip(entry.1).is_some_and(|skip| skip >= keyring - index)
    });
    reaches_gaze && !skips_keyring && starts_keyring_session(&entries)
}

/// How many lines a successful match skips, with `usize::MAX` for the controls that end the
/// auth section. Gaze ships those for the shared stacks, so they are easy to copy into greetd.
pub(super) fn success_skip(control: &str) -> Option<usize> {
    if control == "sufficient" {
        return Some(usize::MAX);
    }
    match control
        .trim_matches(['[', ']'])
        .split_ascii_whitespace()
        .find_map(|field| field.strip_prefix("success="))?
    {
        "done" | "end" => Some(usize::MAX),
        count => count.parse().ok(),
    }
}

pub(super) fn keyring_record_state(
    username: &str,
    backend: gaze_security::keyring::Backend,
) -> Option<bool> {
    if !running_as_root() {
        return None;
    }
    let uid = user_uid(username)?;
    Some(
        Path::new(backend.store_dir())
            .join(format!("{uid}.keyring"))
            .exists(),
    )
}

pub(super) fn check_keyring(report: &mut Report, username: &str, config: Option<&Config>) {
    let Some(config) = config else {
        return;
    };
    if !config.storage.unlock_gnome_keyring {
        report.off(
            "Keyring",
            "GNOME Keyring unlock after a GDM or greetd face login is off",
            format!(
                "Turn it on: set `unlock_gnome_keyring = true` under [storage] in {CONFIG_PATH} \
                 (it also needs `encrypt_templates = true` and [liveness] `enabled = true`), \
                 restart gazed, then run `sudo gaze keyring`."
            ),
        );
        return;
    }

    if let Err(err) = config.storage.validate_keyring(&config.liveness) {
        report.error(
            "Keyring",
            format!("GNOME Keyring unlock is enabled but unusable: {err}"),
            format!(
                "Set `encrypt_templates = true` under [storage] and `enabled = true` under \
                 [liveness] in {CONFIG_PATH}, or turn off `unlock_gnome_keyring`, then restart gazed."
            ),
        );
        return;
    }

    // The distribution ships greetd's stack, so an unedited vendor copy on a machine that does
    // not use greetd is not something the user can or should fix.
    let greetd_stack = read_pam_service(GREETD_PAM_FILE);
    let greetd_in_use = Path::new(GREETD_PAM_FILE).is_file()
        || matches!(
            command_output("systemctl", &["is-active", "greetd"]),
            Ok((true, state)) if state == "active"
        );
    if greetd_in_use
        && greetd_stack
            .as_deref()
            .is_some_and(|contents| !greetd_stack_passes_the_token(contents))
    {
        report.error(
            "Keyring",
            format!("{GREETD_PAM_FILE} does not hand the keyring the authentication token"),
            "Add `use_authtok` to the existing `auth optional pam_gnome_keyring.so` line and \
             keep `session optional pam_gnome_keyring.so auto_start`. The keyring line must come \
             after system-auth, and no pam_gaze.so line above it may end the auth section on a \
             match (`sufficient` or `[success=done ...]`). This file belongs to the distribution, \
             so Gaze does not edit it.",
        );
        return;
    }

    let gdm_face_stack = read_pam_service(&format!("/etc/pam.d/{GDM_FACE_PAM_SERVICE}"));
    match &gdm_face_stack {
        Some(contents) if !gdm_face_stack_passes_the_token(contents) => {
            report.error(
                "Keyring",
                format!(
                    "/etc/pam.d/{GDM_FACE_PAM_SERVICE} does not have the packaged keyring \
                     hand-off and session hook"
                ),
                format!(
                    "This file is preserved across upgrades. Replace it with the packaged stack \
                     (look for /etc/pam.d/{GDM_FACE_PAM_SERVICE}.rpmnew, .pacnew or .dpkg-dist), \
                     or edit it so pam_gaze.so uses `[success=1 default=ignore]` followed by \
                     `auth requisite pam_deny.so` and `auth optional pam_gnome_keyring.so use_authtok`, \
                     plus `session optional pam_gnome_keyring.so auto_start`."
                ),
            );
            return;
        }
        None if greetd_stack.is_none() => {
            report.error(
                "Keyring",
                format!(
                    "GNOME Keyring unlock is enabled but neither /etc/pam.d/{GDM_FACE_PAM_SERVICE} \
                     nor {GREETD_PAM_FILE} exists"
                ),
                "Install the Gaze GNOME extension package, which ships the gdm-face PAM stack, \
                 or set up greetd as described in the greetd guide.",
            );
            return;
        }
        _ => {}
    }

    report_keyring_record(
        report,
        username,
        keyring_record_state(username, gaze_security::keyring::Backend::Gnome),
    );
}

pub(super) fn check_greeter_keyring_selinux(report: &mut Report, config: Option<&Config>) {
    let Some(config) = config else { return };
    let enabled = config.storage.unlock_gnome_keyring || config.storage.unlock_kwallet;
    if !enabled || !selinux::is_enforcing() {
        return;
    }
    report_greeter_keyring_policy(
        report,
        selinux::module_state(selinux::GREETER_KEYRING_MODULE),
    );
}

pub(super) fn report_greeter_keyring_policy(report: &mut Report, policy: ModuleState) {
    const NAME: &str = "Keyring SELinux policy";
    let module = selinux::GREETER_KEYRING_MODULE;
    let fix = format!(
        "Run `sudo semodule -i {}`, then retry the face login.",
        selinux::policy_path(module)
    );
    match policy {
        ModuleState::Loaded => report.pass(
            NAME,
            format!("{module} is loaded, so the login screen can read the keyring record"),
        ),
        ModuleState::NotLoaded => report.error(
            NAME,
            format!(
                "SELinux is enforcing and {module} is not loaded, so the login screen cannot read \
                 the shadow record or the TPM and every face login falls back to the password"
            ),
            fix,
        ),
        ModuleState::NeedsRoot => report.warning(
            NAME,
            format!(
                "SELinux is enforcing, and whether {module} is loaded could not be checked \
                 without root"
            ),
            "Run `sudo gaze doctor` to read the loaded module list.",
        ),
        ModuleState::Unverifiable(why) => report.warning(
            NAME,
            format!("SELinux is enforcing, but the loaded module list could not be read: {why}"),
            format!("Run `semodule -l | grep {module}`; if it prints nothing, {fix}"),
        ),
    }
}

pub(super) fn report_keyring_record(report: &mut Report, username: &str, state: Option<bool>) {
    match state {
        Some(true) => report.pass(
            "Keyring",
            format!("a TPM-protected keyring credential is enrolled for {username}"),
        ),
        Some(false) => report.warning(
            "Keyring",
            format!("GNOME Keyring unlock is enabled but {username} has no enrolled credential"),
            format!("Run `sudo gaze keyring --user {username}`."),
        ),
        None => report.warning(
            "Keyring",
            format!(
                "the {GDM_FACE_PAM_SERVICE} stack passes the token, but whether {username} has \
                 an enrolled credential could not be checked without root"
            ),
            "Run `sudo gaze doctor` to check the credential record.",
        ),
    }
}

/// Check the exact managed branch: no wallet hook is reachable on biometric failure.
pub(super) fn kde_login_stack_passes_the_token(contents: &str) -> bool {
    let entries: Vec<_> = contents.lines().filter_map(pam_entry).collect();
    let auth: Vec<_> = entries.iter().filter(|entry| entry.0 == "auth").collect();
    let handoff = auth.windows(4).any(|lines| {
        let (_, control, module, options) = *lines[0];
        module == "pam_gaze.so"
            && (control
                .split_ascii_whitespace()
                .eq(["[success=1", "default=ignore]"])
                || control
                    .split_ascii_whitespace()
                    .eq(["[success=1", "default=die]"]))
            && options.split_ascii_whitespace().eq(["kde-login"])
            && lines[1]
                .1
                .split_ascii_whitespace()
                .eq(["[success=2", "default=ignore]"])
            && lines[1].2 == "pam_permit.so"
            && lines[2].1 == "optional"
            && lines[2].2 == "pam_kwallet5.so"
            && lines[2].3.is_empty()
            && lines[3]
                .1
                .split_ascii_whitespace()
                .eq(["[success=done", "default=ignore]"])
            && lines[3].2 == "pam_permit.so"
    });
    handoff
        && entries.iter().any(|&(kind, control, module, options)| {
            kind == "session"
                && control == "optional"
                && module == "pam_kwallet5.so"
                && options.split_ascii_whitespace().eq(["auto_start"])
        })
}

pub(super) fn check_kwallet(report: &mut Report, username: &str, config: Option<&Config>) {
    let Some(config) = config else { return };
    if !config.storage.unlock_kwallet {
        report.off("KWallet", "KWallet unlock after a KDE face login is off",
            "Enable KWallet unlock in `gaze config`, then run `gaze keyring --kwallet` and `sudo gaze-kde-pam enable-login`.");
        return;
    }
    if let Err(err) = config.storage.validate_keyring(&config.liveness) {
        report.error("KWallet", format!("KWallet unlock is enabled but unusable: {err}"),
            "Enable TPM template encryption and liveness, or disable KWallet unlock in `gaze config`.");
        return;
    }
    if !pam_search_dirs()
        .iter()
        .any(|dir| dir.join("pam_kwallet5.so").exists())
    {
        report.warning(
            "KWallet",
            "pam_kwallet5.so was not found",
            "Install your distribution's KWallet PAM package (kwallet-pam or libpam-kwallet5).",
        );
    }
    let mut found = false;
    for service in ["sddm", "plasmalogin", "plasmalogin-fingerprint"] {
        let Some(contents) = read_pam_service(&format!("/etc/pam.d/{service}")) else {
            continue;
        };
        found = true;
        if !kde_login_stack_passes_the_token(&contents) {
            report.warning("KWallet", format!("{service} lacks the managed KWallet handoff/session hook"),
                "Run `sudo gaze-kde-pam enable-login`. Custom PAM entries must use sequential mode and pass the token to pam_kwallet5 before ending authentication.");
        }
    }
    if !found {
        report.error(
            "KWallet",
            "No supported KDE login PAM service was found",
            "Install SDDM or Plasma Login Manager and run `sudo gaze-kde-pam enable-login`.",
        );
        return;
    }
    match keyring_record_state(username, gaze_security::keyring::Backend::KWallet) {
        Some(true) => report.pass(
            "KWallet",
            format!("a TPM-protected KWallet credential is enrolled for {username}"),
        ),
        Some(false) => report.warning(
            "KWallet",
            format!("{username} has no enrolled KWallet credential"),
            format!("Run `sudo gaze keyring --kwallet --user {username}`."),
        ),
        None => report.warning(
            "KWallet",
            "KWallet enrollment could not be checked without root",
            "Run `sudo gaze doctor` to check the credential record.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_keyring_policy_is_only_an_error_once_the_module_store_was_read() {
        let reported = |policy| {
            let mut report = Report::default();
            report_greeter_keyring_policy(&mut report, policy);
            let check = report
                .checks
                .into_iter()
                .find(|check| check.name == "Keyring SELinux policy")
                .expect("the keyring SELinux check always reports once it runs");
            (check.level, check.message, check.fix.unwrap_or_default())
        };

        let (level, _, _) = reported(ModuleState::Loaded);
        assert_eq!(level, Level::Pass);

        let (level, _, fix) = reported(ModuleState::NotLoaded);
        assert_eq!(level, Level::Error);
        assert!(
            fix.contains("semodule -i /usr/share/gaze/gaze-greeter-keyring.pp"),
            "the fix names the shipped module: {fix}"
        );

        let (level, message, fix) = reported(ModuleState::NeedsRoot);
        assert_eq!(level, Level::Warning);
        assert!(
            !message.contains("is not loaded"),
            "an unchecked module must not be reported as absent: {message}"
        );
        assert!(
            fix.contains("sudo gaze doctor"),
            "the fix is to re-run as root, not to load the module: {fix}"
        );

        let (level, _, _) = reported(ModuleState::Unverifiable("broken".into()));
        assert_eq!(level, Level::Warning);
    }

    #[test]
    fn keyring_enrollment_that_could_not_be_read_is_not_a_checkmark() {
        let reported = |state| {
            let mut report = Report::default();
            report_keyring_record(&mut report, "lambros", state);
            let check = report
                .checks
                .into_iter()
                .find(|check| check.name == "Keyring")
                .expect("the keyring record always reports");
            (check.level, check.message, check.fix.unwrap_or_default())
        };

        let (level, _, _) = reported(Some(true));
        assert_eq!(level, Level::Pass);

        let (level, _, _) = reported(Some(false));
        assert_eq!(level, Level::Warning);

        let (level, message, fix) = reported(None);
        assert_eq!(
            level,
            Level::Warning,
            "an unprivileged run never checked the record, so it cannot pass it"
        );
        assert!(
            message.contains("without root"),
            "say which half of the check ran: {message}"
        );
        assert!(fix.contains("sudo gaze doctor"), "{fix}");
    }

    #[test]
    fn kwallet_diagnostics_reject_bypassed_or_unsafe_handoffs() {
        let valid = "-auth [success=1 default=ignore] pam_gaze.so kde-login\n\
            -auth [success=2 default=ignore] pam_permit.so\n\
            -auth optional pam_kwallet5.so\n\
            -auth [success=done default=ignore] pam_permit.so\n\
            -session optional pam_kwallet5.so auto_start\n";
        assert!(kde_login_stack_passes_the_token(valid));
        assert!(kde_login_stack_passes_the_token(&valid.replacen(
            "default=ignore",
            "default=die",
            1
        )));
        for invalid in [
            valid.replace("success=1", "success=done"),
            valid.replace("success=2", "success=1"),
            valid.replace("kde-login", "simultaneous"),
            valid.replace("-auth optional pam_kwallet5.so\n", ""),
            valid.replace("-session optional pam_kwallet5.so auto_start\n", ""),
        ] {
            assert!(!kde_login_stack_passes_the_token(&invalid));
        }
    }

    #[test]
    fn every_shipped_gdm_face_stack_passes_the_keyring_token() {
        for template in ["gdm-face", "gdm-face.arch", "gdm-face.deb", "gdm-face.suse"] {
            let path =
                concat!(env!("CARGO_MANIFEST_DIR"), "/../../packaging/pam/").to_string() + template;
            let contents = std::fs::read_to_string(&path).expect(template);
            assert!(
                gdm_face_stack_passes_the_token(&contents),
                "{template} must hand the token to pam_gnome_keyring"
            );
        }
    }

    #[test]
    fn incomplete_or_misordered_keyring_stacks_are_not_reported_healthy() {
        let valid = "auth [success=1 default=ignore] /usr/lib/security/pam_gaze.so\n\
            auth requisite pam_deny.so\n\
            auth optional pam_gnome_keyring.so use_authtok\n\
            session optional pam_gnome_keyring.so auto_start\n";
        assert!(gdm_face_stack_passes_the_token(valid));
        for broken in [
            valid.replace(
                "auth [success=1 default=ignore] /usr/lib/security/pam_gaze.so\n",
                "",
            ),
            valid.replace("[success=1 default=ignore]", "sufficient"),
            valid.replace("pam_gaze.so", "pam_gaze.so simultaneous"),
            valid.replace("requisite pam_deny.so", "optional pam_deny.so"),
            valid.replace("auth requisite", "@include common-auth\nauth requisite"),
            valid.replace("use_authtok", "not_use_authtok"),
            valid.replace("use_authtok", "use_authtok only_if=login"),
            valid.replace("session optional pam_gnome_keyring.so auto_start\n", ""),
            valid.replace("session optional", "# session optional"),
            valid.replace("auto_start", "auto_start only_if=login"),
            format!(
                "auth optional pam_gnome_keyring.so use_authtok\n{}",
                valid.replace("auth optional pam_gnome_keyring.so use_authtok\n", "")
            ),
        ] {
            assert!(!gdm_face_stack_passes_the_token(&broken), "{broken}");
        }
    }

    #[test]
    fn an_upgrade_preserved_gdm_face_stack_is_detected_as_stale() {
        let stale = "auth required pam_env.so\n\
             auth [success=done ignore=ignore default=bad] pam_gaze.so\n\
             auth optional pam_gnome_keyring.so only_if=login auto_start\n\
             auth required pam_deny.so\n";
        assert!(!gdm_face_stack_passes_the_token(stale));

        let no_keyring_module = "auth required pam_env.so\n\
             auth [success=1 default=ignore] pam_gaze.so\n\
             auth requisite pam_deny.so\n";
        assert!(!gdm_face_stack_passes_the_token(no_keyring_module));

        let commented_out = "auth [success=1 default=ignore] pam_gaze.so\n\
             auth requisite pam_deny.so\n\
             # auth optional pam_gnome_keyring.so use_authtok\n";
        assert!(
            !gdm_face_stack_passes_the_token(commented_out),
            "a commented-out keyring line must not count"
        );
    }

    #[test]
    fn the_greetd_keyring_line_must_follow_the_stack_that_runs_gaze() {
        let valid = "auth substack system-auth\n\
            auth optional pam_gnome_keyring.so use_authtok\n\
            session optional pam_gnome_keyring.so auto_start\n";
        assert!(greetd_stack_passes_the_token(valid));
        assert!(greetd_stack_passes_the_token(
            &valid.replace("session optional", "session required")
        ));
        for jumps_the_password_step in [
            "auth [success=1 default=ignore] pam_gaze.so\n\
             auth substack system-auth\n\
             auth optional pam_gnome_keyring.so use_authtok\n\
             session optional pam_gnome_keyring.so auto_start\n",
            "auth [success=2 default=ignore] pam_gaze.so\n\
             auth substack system-auth\n\
             auth requisite pam_deny.so\n\
             auth optional pam_gnome_keyring.so use_authtok\n\
             session optional pam_gnome_keyring.so auto_start\n",
        ] {
            assert!(
                greetd_stack_passes_the_token(jumps_the_password_step),
                "{jumps_the_password_step}"
            );
        }

        let fedora = "auth       substack    system-auth\n\
            auth       optional    pam_gnome_keyring.so use_authtok\n\
            -auth       optional    pam_kwallet5.so\n\
            -auth       optional    pam_kwallet.so\n\
            auth       include     postlogin\n\
            account    required    pam_sepermit.so\n\
            account    include     system-auth\n\
            session    optional    pam_keyinit.so force revoke\n\
            session    include     system-auth\n\
            session    optional    pam_gnome_keyring.so auto_start\n\
            session    include     postlogin\n";
        assert!(greetd_stack_passes_the_token(fedora));
        assert!(!greetd_stack_passes_the_token(
            &fedora.replace("use_authtok", "")
        ));

        for broken in [
            "auth [success=1 default=ignore] pam_gaze.so\n\
             auth optional pam_gnome_keyring.so use_authtok\n\
             session optional pam_gnome_keyring.so auto_start\n",
            "auth sufficient pam_gaze.so\n\
             auth sufficient pam_unix.so nullok\n\
             auth optional pam_gnome_keyring.so use_authtok\n\
             session optional pam_gnome_keyring.so auto_start\n",
            "auth [success=done default=ignore] pam_gaze.so\n\
             auth include system-auth\n\
             auth optional pam_gnome_keyring.so use_authtok\n\
             session optional pam_gnome_keyring.so auto_start\n",
            "auth [success=end default=ignore] pam_gaze.so\n\
             auth include system-auth\n\
             auth optional pam_gnome_keyring.so use_authtok\n\
             session optional pam_gnome_keyring.so auto_start\n",
            "auth optional pam_gnome_keyring.so use_authtok\n\
             auth substack system-auth\n\
             session optional pam_gnome_keyring.so auto_start\n",
        ] {
            assert!(!greetd_stack_passes_the_token(broken), "{broken}");
        }
        for broken in [
            valid.replace("use_authtok", "only_if=login"),
            valid.replace("use_authtok", ""),
            valid.replace("auto_start", "auto_start only_if=login"),
            valid.replace("session optional pam_gnome_keyring.so auto_start\n", ""),
            valid.replace("session optional", "# session optional"),
        ] {
            assert!(!greetd_stack_passes_the_token(&broken), "{broken}");
        }
    }
}
