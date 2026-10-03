// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) fn desktop_name() -> String {
    let from_env = [
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        std::env::var("XDG_SESSION_DESKTOP").unwrap_or_default(),
        std::env::var("DESKTOP_SESSION").unwrap_or_default(),
    ]
    .join(":")
    .to_ascii_lowercase();
    if from_env.chars().any(|c| c != ':') {
        return from_env;
    }
    // `sudo` strips those, so fall back to what is running: otherwise
    // `sudo gaze doctor` silently drops every desktop check.
    desktop_from_processes(owning_uid())
}

pub(super) fn running_as_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// The user whose session is being checked: the invoking user under `sudo`.
pub(super) fn owning_uid() -> u32 {
    std::env::var("SUDO_UID")
        .ok()
        .and_then(|uid| uid.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getuid() })
}

/// Names only, joined like the environment variables above so the callers'
/// `contains` checks are unchanged. The CLI does not link `pam-gaze`.
pub(super) fn desktop_from_processes(uid: u32) -> String {
    use std::os::unix::fs::MetadataExt;

    let Ok(entries) = fs::read_dir("/proc") else {
        return String::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !entry.metadata().is_ok_and(|meta| meta.uid() == uid) {
            continue;
        }
        let Ok(comm) = fs::read_to_string(path.join("comm")) else {
            continue;
        };
        let name = match comm.trim() {
            "plasmashell" | "kwin_wayland" | "kwin_x11" => "kde",
            "gnome-shell" => "gnome",
            "Hyprland" | "hyprland" => "hyprland",
            _ => continue,
        };
        if !found.contains(&name) {
            found.push(name);
        }
    }
    found.join(":")
}

/// The prefs window has one page, Behavior, with two groups; naming the exact
/// path beats "the extension preferences", which sends people hunting.
pub(super) fn gnome_prefs_path(group: &str, switch: &str) -> String {
    format!(
        "Open it with `gnome-extensions prefs {GNOME_EXTENSION_ID}` (or the Extensions app, then Gaze), then Behavior -> {group} -> \"{switch}\""
    )
}

/// GNOME Shell only scans extension directories at session start, so a session asked
/// to enable a UUID it never scanned drops it at the next `enabled-extensions` rewrite.
pub(super) fn gnome_extension_enable_steps() -> String {
    format!(
        "1. Reboot, or log out and back in, so GNOME Shell scans the extension.\n\
         2. Run `gnome-extensions enable {GNOME_EXTENSION_ID}`.\n\
         3. Run `gsettings set {GNOME_EXTENSION_SCHEMA} enable-face-authentication true`.\n\
         If step 2 reports that the extension does not exist, Shell has not rescanned yet: reboot and repeat.\n\
         Details: {GNOME_DOCS_URL}"
    )
}

pub(super) fn check_desktop_integration(report: &mut Report) {
    let desktop = desktop_name();
    if desktop.contains("gnome") {
        match command_output("gnome-extensions", &["list", "--enabled"]) {
            Ok((true, output)) if output.lines().any(|line| line.trim() == GNOME_EXTENSION_ID) => {
                report.pass("GNOME extension", "enabled for the current user");
            }
            Ok((true, _)) if extension_installed() => report.warning(
                "GNOME extension",
                "installed, but not enabled for the current user",
                gnome_extension_enable_steps(),
            ),
            Ok((true, _)) => report.warning(
                "GNOME extension",
                "not installed for the current user",
                format!(
                    "Install the Gaze GNOME extension package (`gaze-gnome-extension`), reboot, then run `gnome-extensions enable {GNOME_EXTENSION_ID}`. See {GNOME_DOCS_URL}"
                ),
            ),
            Ok((false, message)) => report.warning(
                "GNOME extension",
                format!("could not query extensions: {message}"),
                "Verify GNOME Shell is running and reinstall the Gaze GNOME extension package.",
            ),
            Err(err) => report.warning(
                "GNOME extension",
                format!("could not query extensions: {err}"),
                "Install the Gaze GNOME extension package for lock-screen authentication.",
            ),
        }

        match extension_setting("enable-face-authentication") {
            Ok((true, value)) if value == "true" => {
                report.pass(
                    "GNOME lock-screen face auth",
                    "enabled for the current user",
                );
            }
            Ok((true, _)) => report.off(
                "GNOME lock-screen face auth",
                "off for the current user, so the lock screen only takes your password",
                format!(
                    "Turn it on: {}.\n\
                     From a terminal: `dconf write /org/gnome/shell/extensions/gaze/enable-face-authentication true`.\n\
                     (`gsettings set {GNOME_EXTENSION_SCHEMA} ...` does the same, but cannot find the schema where it ships inside the extension directory, as on NixOS.)",
                    gnome_prefs_path("Face authentication", "Enable face authentication (lock screen)")
                ),
            ),
            Ok((false, message)) => report.warning(
                "GNOME lock-screen face auth",
                format!("could not read the extension setting: {message}"),
                "Reinstall the Gaze GNOME extension package.",
            ),
            Err(err) => report.warning(
                "GNOME lock-screen face auth",
                format!("could not read the extension setting: {err}"),
                "Reinstall the Gaze GNOME extension package.",
            ),
        }

        let override_exists = Path::new(GDM_FACE_OVERRIDE_PATH).exists();
        let dconf_face_auth = gdm_face_auth_from_dconf();
        match (dconf_face_auth, override_exists) {
            (Some(false), true) => report.warning(
                "GDM login face auth",
                format!(
                    "{GDM_FACE_OVERRIDE_PATH} enables it, but the compiled GDM dconf database still reports it disabled"
                ),
                "Run `sudo dconf update`, then restart GDM (or reboot).",
            ),
            (Some(true), false) => report.pass(
                "GDM login face auth",
                "enabled in the GDM dconf profile by your system configuration, not by Gaze (on NixOS, `services.gaze.gnome.gdmFaceLogin`)",
            ),
            (_, true) => match gdm_greeter_readiness() {
                GdmGreeterReadiness::Ready => report.pass(
                    "GDM login face auth",
                    format!(
                        "enabled system-wide via {GDM_FACE_OVERRIDE_PATH}; toggle it under Behavior -> GDM login screen in `gnome-extensions prefs {GNOME_EXTENSION_ID}`"
                    ),
                ),
                GdmGreeterReadiness::ProfileMissingSystemDb => report.error(
                    "GDM login face auth",
                    format!(
                        "{GDM_FACE_OVERRIDE_PATH} exists, but {GDM_DCONF_PROFILE_PATH} does not list `system-db:{GDM_DCONF_PROFILE}`, so GDM never reads it"
                    ),
                    format!(
                        "Add a `system-db:{GDM_DCONF_PROFILE}` line to {GDM_DCONF_PROFILE_PATH}, run `sudo dconf update`, then reboot."
                    ),
                ),
                GdmGreeterReadiness::CompiledDbMissing => report.error(
                    "GDM login face auth",
                    format!(
                        "{GDM_FACE_OVERRIDE_PATH} exists, but the compiled database {GDM_COMPILED_DB_PATH} does not"
                    ),
                    "Run `sudo dconf update`, then reboot.",
                ),
                GdmGreeterReadiness::ExtensionNotEnabled => report.error(
                    "GDM login face auth",
                    format!(
                        "the GDM database does not enable {GNOME_EXTENSION_ID} for the greeter, so the login screen never starts the {GDM_FACE_PAM_SERVICE} PAM service"
                    ),
                    "Reinstall the Gaze GNOME extension package, run `sudo dconf update`, then reboot.",
                ),
                GdmGreeterReadiness::ExtensionsDisabled(source) => report.error(
                    "GDM login face auth",
                    format!(
                        "the greeter resolves `org.gnome.shell disable-user-extensions` to true, which switches off every GNOME Shell extension at the login screen, {GNOME_EXTENSION_ID} included"
                    ),
                    match source {
                        Some(path) => format!(
                            "{} holds that key and outranks every keyfile under /etc/dconf/db/gdm.d, so it has to be cleared there:\n\
                             sudo rm -f {}\n\
                             Then reboot. GDM writes the file again with its own defaults.",
                            path.display(),
                            path.display()
                        ),
                        None => format!(
                            "Put `disable-user-extensions=false` under `[org/gnome/shell]` in {GDM_FACE_OVERRIDE_PATH}, run `sudo dconf update`, then reboot."
                        ),
                    },
                ),
                GdmGreeterReadiness::Unverifiable(why) => report.warning(
                    "GDM login face auth",
                    format!(
                        "{GDM_FACE_OVERRIDE_PATH} enables it, but the greeter configuration could not be verified: {why}"
                    ),
                    "Install the `dconf` command-line tool and re-run `gaze doctor`.",
                ),
            },
            (_, false) => report.off(
                "GDM login face auth",
                "off, so the login screen only takes your password (the lock screen is a separate switch)",
                format!(
                    "Turn it on: {}, then reboot. It asks for admin authorization and writes {GDM_FACE_OVERRIDE_PATH} for you.\n\
                     By hand: put `enable-face-authentication=true` under `[org/gnome/shell/extensions/gaze]` in {GDM_FACE_OVERRIDE_PATH}, run `sudo dconf update`, then reboot.\n\
                     Details: {GNOME_DOCS_URL}#optional-enable-face-at-gdm-login",
                    gnome_prefs_path("GDM login screen", "Enable face auth at GDM login")
                ),
            ),
        }

        if dconf_face_auth == Some(true) || override_exists {
            check_gdm_selinux(report);
        }
    }

    let omarchy = desktop.contains("hyprland")
        && Path::new("/usr/share/omarchy/shell/plugins/lock/manifest.json").exists();
    if omarchy {
        match command_output("gaze-omarchy", &["doctor"]) {
            Ok((true, output)) => report.pass("Omarchy lock", output),
            Ok((false, output)) => report.warning(
                "Omarchy lock",
                output,
                "Run `gaze-omarchy enable` from your unlocked desktop. See https://gaze.gundulabs.com/guide/omarchy",
            ),
            Err(_) => report.warning(
                "Omarchy lock",
                "Gaze Omarchy integration is not installed",
                "Install `gaze-omarchy` (`gaze-omarchy-bin` on Arch), then run `gaze-omarchy enable` without sudo.",
            ),
        }
    }
    if desktop.contains("hyprland") && !omarchy {
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
        let config_path = config_home.map(|home| home.join("hypr/hyprlock.conf"));
        let configured = config_path
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .is_some_and(|contents| hyprlock_selects_gaze(&contents));
        if configured {
            report.pass("hyprlock", "configured to use a Gaze PAM service");
        } else {
            report.warning(
                "hyprlock",
                "the current user's hyprlock.conf does not select a Gaze PAM service",
                "Set `module = hyprlock-gaze` in the hyprlock `auth { pam { ... } }` block.",
            );
        }
    }

    if desktop.contains("kde") || desktop.contains("plasma") {
        check_kde_lock_screen(
            report,
            read_pam_service(KDE_FACE_PAM_FILE).as_deref(),
            read_pam_service(KDE_SMARTCARD_PAM_FILE).as_deref(),
        );
        check_kde_login_greeter(
            report,
            read_pam_service(PLASMALOGIN_FACE_PAM_FILE).as_deref(),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KdeLockStatus {
    Wired,
    /// The simultaneous module, which deadlocks on this slot.
    Grosshack,
    NotWired,
    /// KScreenLocker has no biometric slot configured at all.
    NoService,
}

pub(super) fn slot_status(slot: Option<&str>) -> KdeLockStatus {
    let Some(contents) = slot else {
        return KdeLockStatus::NoService;
    };
    let auth_lines = || {
        contents.lines().filter(|line| {
            matches!(
                line.split('#')
                    .next()
                    .unwrap_or_default()
                    .split_whitespace()
                    .next(),
                // `-auth` is what the helper writes: a missing module is then
                // skipped instead of aborting the greeter's stack.
                Some("auth") | Some("-auth")
            )
        })
    };
    if auth_lines().any(|line| {
        line.contains("pam_gaze_grosshack.so")
            || (pam_line_has_reference(line)
                && line.split_whitespace().any(|tok| tok == "simultaneous"))
    }) {
        return KdeLockStatus::Grosshack;
    }
    if auth_lines().any(pam_line_has_reference) {
        return KdeLockStatus::Wired;
    }
    KdeLockStatus::NotWired
}

/// Either slot the greeter starts up front will do, so report on whichever has
/// the most to say: one being wired is a pass however the other looks.
pub(super) fn kde_lock_status(
    kde_fingerprint: Option<&str>,
    kde_smartcard: Option<&str>,
) -> (KdeLockStatus, &'static str) {
    let slots = [
        (KDE_FACE_PAM_FILE, slot_status(kde_fingerprint)),
        (KDE_SMARTCARD_PAM_FILE, slot_status(kde_smartcard)),
    ];
    for wanted in [
        KdeLockStatus::Wired,
        KdeLockStatus::Grosshack,
        KdeLockStatus::NotWired,
    ] {
        if let Some((file, status)) = slots.iter().find(|(_, status)| *status == wanted) {
            return (*status, file);
        }
    }
    (KdeLockStatus::NoService, KDE_FACE_PAM_FILE)
}

pub(super) fn check_kde_lock_screen(
    report: &mut Report,
    kde_fingerprint: Option<&str>,
    kde_smartcard: Option<&str>,
) {
    const NAME: &str = "KDE lock screen";
    let (status, file) = kde_lock_status(kde_fingerprint, kde_smartcard);
    let slot = file.trim_start_matches("/etc/pam.d/");
    match status {
        KdeLockStatus::Wired => report.pass(
            NAME,
            format!(
                "{slot} runs Gaze, so face unlock starts on its own next to the password field"
            ),
        ),
        KdeLockStatus::Grosshack => report.warning(
            NAME,
            format!("{slot} runs pam_gaze.so in simultaneous mode, which waits for a password prompt that KScreenLocker can never answer"),
            format!("Use sequential mode there: replace it with `-auth [success=done default=ignore] pam_gaze.so` in {file}, or reinstall gaze-kde."),
        ),
        KdeLockStatus::NotWired => report.warning(
            NAME,
            format!("{file} does not run Gaze, so face auth only starts after you submit the password field"),
            "Install the gaze-kde package, or run `sudo gaze-kde-pam enable`.",
        ),
        KdeLockStatus::NoService => report.warning(
            NAME,
            format!("{file} does not exist, so KScreenLocker has no biometric slot to start"),
            "Install the gaze-kde package, or run `sudo gaze-kde-pam enable`, which creates it.",
        ),
    }
}

/// The greeter only scans before you type where it starts a service of its own.
/// Everywhere else face auth waits for the submit, as a fingerprint reader does.
pub(super) fn check_kde_login_greeter(report: &mut Report, plasmalogin_face: Option<&str>) {
    const NAME: &str = "KDE login greeter";
    match plasmalogin_face {
        None => report.pass(
            NAME,
            "no up-front biometric service upstream, so face auth runs when you submit the login form (press Enter on an empty password field)",
        ),
        Some(contents) if slot_status(Some(contents)) == KdeLockStatus::Wired => report.pass(
            NAME,
            "plasmalogin-fingerprint runs Gaze, so face auth starts as soon as the greeter shows your user",
        ),
        Some(_) => report.warning(
            NAME,
            format!("{PLASMALOGIN_FACE_PAM_FILE} exists but does not run Gaze, so face auth at the greeter waits for you to submit the form"),
            "Run `sudo gaze-kde-pam enable-login` to scan before you type.",
        ),
    }
}

/// The KDE biometric slots start without anything to route a response back, so
/// `require_confirmation_lock_screen` is silently ignored there by design:
/// prompting would hang the slot for the rest of the lock rather than ask
/// anybody anything. Say so when the toggle is on and a slot is wired, instead
/// of letting the setting imply a confirmation that never happens.
pub(super) fn check_kde_confirmation_bypass(
    report: &mut Report,
    config: Option<&Config>,
    kde_fingerprint: Option<&str>,
    kde_smartcard: Option<&str>,
    plasmalogin_face: Option<&str>,
) {
    const NAME: &str = "KDE confirmation";
    let Some(config) = config else {
        return;
    };
    if !config.auth.require_confirmation_lock_screen {
        return;
    }
    let mut bypassed = Vec::new();
    if slot_status(kde_fingerprint) == KdeLockStatus::Wired {
        bypassed.push(KDE_FACE_PAM_FILE.trim_start_matches("/etc/pam.d/"));
    }
    if slot_status(kde_smartcard) == KdeLockStatus::Wired {
        bypassed.push(KDE_SMARTCARD_PAM_FILE.trim_start_matches("/etc/pam.d/"));
    }
    if plasmalogin_face.is_some_and(|contents| slot_status(Some(contents)) == KdeLockStatus::Wired)
    {
        bypassed.push(PLASMALOGIN_FACE_PAM_FILE.trim_start_matches("/etc/pam.d/"));
    }
    if bypassed.is_empty() {
        return;
    }
    report.warning(
        NAME,
        format!(
            "require_confirmation_lock_screen is on, but {} cannot be prompted, so a face match unlocks without confirmation there",
            bypassed.join(", ")
        ),
        "This is by design: the greeter never delivers a response to a noninteractive slot, so asking would hang it for the rest of the lock. Leave the toggle for surfaces that can prompt (sudo with a TTY, polkit, GNOME), or turn it off if the KDE bypass surprises you. See the KDE guide.",
    );
}

pub(super) fn hyprlock_selects_gaze(contents: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.split('#').next().unwrap_or_default();
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        matches!(key.trim(), "module" | "pam_module") && value.trim().starts_with("hyprlock-gaze")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprlock_modern_pam_module_key_is_detected() {
        let contents = "auth {\n    pam {\n        module = hyprlock-gaze\n    }\n}\n";
        assert!(hyprlock_selects_gaze(contents));
    }

    #[test]
    fn hyprlock_legacy_pam_module_key_is_detected() {
        let contents = "general {\n    pam_module = hyprlock-gaze-simultaneous\n}\n";
        assert!(hyprlock_selects_gaze(contents));
    }

    #[test]
    fn hyprlock_without_gaze_is_not_detected() {
        let contents = "auth {\n    pam {\n        module = hyprlock\n    }\n}\n";
        assert!(!hyprlock_selects_gaze(contents));
    }

    #[test]
    fn hyprlock_commented_out_module_is_not_detected() {
        let contents = "auth {\n    pam {\n        # module = hyprlock-gaze\n    }\n}\n";
        assert!(!hyprlock_selects_gaze(contents));
    }

    #[test]
    fn slot_status_reads_the_auth_stack() {
        assert_eq!(
            slot_status(Some(
                "#%PAM-1.0\nauth        [success=done default=ignore]                pam_gaze.so"
            )),
            KdeLockStatus::Wired
        );
        assert_eq!(
            slot_status(Some(
                "auth required pam_fprintd.so\nauth sufficient pam_gaze.so"
            )),
            KdeLockStatus::Wired
        );
        assert_eq!(
            slot_status(Some("auth sufficient pam_gaze.so simultaneous")),
            KdeLockStatus::Grosshack
        );
        assert_eq!(
            slot_status(Some("auth sufficient pam_gaze_grosshack.so")),
            KdeLockStatus::Grosshack
        );
        assert_eq!(
            slot_status(Some(
                "auth required pam_fprintd.so\nauth required pam_deny.so"
            )),
            KdeLockStatus::NotWired
        );
        assert_eq!(
            slot_status(Some("# auth sufficient pam_gaze.so")),
            KdeLockStatus::NotWired
        );
        assert_eq!(
            slot_status(Some("session optional pam_gaze.so")),
            KdeLockStatus::NotWired
        );
        assert_eq!(slot_status(None), KdeLockStatus::NoService);

        // What gaze-kde-pam actually writes: `-` so a missing module is skipped.
        assert_eq!(
            slot_status(Some(
                "-auth       [success=done default=ignore]                pam_gaze.so"
            )),
            KdeLockStatus::Wired,
            "the reported state must match the line the helper installs"
        );
    }

    #[test]
    fn either_biometric_slot_counts_as_wired() {
        let reader = Some("auth required pam_fprintd.so");
        let gaze = Some("auth [success=done default=ignore] pam_gaze.so");

        assert_eq!(
            kde_lock_status(gaze, reader),
            (KdeLockStatus::Wired, KDE_FACE_PAM_FILE)
        );
        assert_eq!(
            kde_lock_status(reader, gaze),
            (KdeLockStatus::Wired, KDE_SMARTCARD_PAM_FILE),
            "the smartcard slot is a first-class home for Gaze"
        );
        assert_eq!(
            kde_lock_status(reader, None),
            (KdeLockStatus::NotWired, KDE_FACE_PAM_FILE)
        );
        assert_eq!(
            kde_lock_status(None, None),
            (KdeLockStatus::NoService, KDE_FACE_PAM_FILE)
        );
        assert_eq!(
            kde_lock_status(Some("auth sufficient pam_gaze_grosshack.so"), reader),
            (KdeLockStatus::Grosshack, KDE_FACE_PAM_FILE),
            "a deadlocking module must be reported over a merely unwired slot"
        );
    }

    #[test]
    fn kde_lock_screen_check_warns_unless_the_plain_module_is_wired() {
        let level = |fingerprint: Option<&str>, smartcard: Option<&str>| {
            let mut report = Report::default();
            check_kde_lock_screen(&mut report, fingerprint, smartcard);
            report
                .checks
                .iter()
                .find(|check| check.name == "KDE lock screen")
                .map(|check| check.level)
                .expect("the KDE lock screen check always reports")
        };

        assert_eq!(
            level(Some("auth sufficient pam_gaze.so"), None),
            Level::Pass
        );
        assert_eq!(
            level(
                Some("auth required pam_fprintd.so"),
                Some("auth sufficient pam_gaze.so")
            ),
            Level::Pass
        );
        assert_eq!(
            level(Some("auth sufficient pam_gaze.so simultaneous"), None),
            Level::Warning
        );
        assert_eq!(
            level(Some("auth sufficient pam_gaze_grosshack.so"), None),
            Level::Warning
        );
        assert_eq!(
            level(Some("auth required pam_fprintd.so"), None),
            Level::Warning
        );
        assert_eq!(level(None, None), Level::Warning);
    }

    #[test]
    fn kde_confirmation_bypass_is_reported_when_the_toggle_is_on_and_a_slot_is_wired() {
        let check = |confirmation: bool,
                     fingerprint: Option<&str>,
                     smartcard: Option<&str>,
                     face: Option<&str>| {
            let mut config = Config::default();
            config.auth.require_confirmation_lock_screen = confirmation;
            let mut report = Report::default();
            check_kde_confirmation_bypass(&mut report, Some(&config), fingerprint, smartcard, face);
            report
                .checks
                .iter()
                .find(|check| check.name == "KDE confirmation")
                .map(|check| (check.level, check.message.clone()))
        };

        // Off means nothing to say, even when a slot is wired.
        assert!(check(false, Some("auth sufficient pam_gaze.so"), None, None).is_none());
        // On with nothing wired means nothing is bypassed.
        assert!(check(true, None, None, None).is_none());
        assert!(check(true, Some("auth required pam_fprintd.so"), None, None).is_none());

        let (level, message) = check(true, Some("auth sufficient pam_gaze.so"), None, None)
            .expect("a wired slot with confirmation on must warn");
        assert_eq!(level, Level::Warning);
        assert!(message.contains("kde-fingerprint"), "{message}");
        assert!(
            message.contains("require_confirmation_lock_screen"),
            "{message}"
        );

        let (_, message) = check(
            true,
            None,
            Some("auth sufficient pam_gaze.so"),
            Some("auth sufficient pam_gaze.so"),
        )
        .expect("both smartcard and greeter slots must warn");
        assert!(message.contains("kde-smartcard"), "{message}");
        assert!(message.contains("plasmalogin-fingerprint"), "{message}");
    }

    #[test]
    fn login_greeter_check_only_complains_about_an_unused_slot() {
        let level = |contents: Option<&str>| {
            let mut report = Report::default();
            check_kde_login_greeter(&mut report, contents);
            report
                .checks
                .iter()
                .find(|check| check.name == "KDE login greeter")
                .map(|check| check.level)
                .expect("the KDE login greeter check always reports")
        };

        // Nothing to wire is the normal state today, not a problem to fix.
        assert_eq!(level(None), Level::Pass);
        assert_eq!(level(Some("auth sufficient pam_gaze.so")), Level::Pass);
        assert_eq!(level(Some("auth required pam_fprintd.so")), Level::Warning);
    }
}
