// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::selinux::{self, ModuleState};
use console::{Term, style};
use gaze_core::config::{
    CONFIG_PATH, Config, MAX_LIVENESS_MAX_SECONDS, MAX_LIVENESS_THRESHOLD,
    MIN_LIVENESS_MAX_SECONDS, MIN_LIVENESS_THRESHOLD, SecurityField, unknown_config_keys,
};
use gaze_core::dbus::{
    GazeProxy, dbus_error_message, dbus_is_file_not_found, dbus_is_not_activatable,
    try_benchmark_from_daemon,
};
use gaze_core::desktop::{
    GDM_DCONF_FACE_AUTH_KEY, GDM_DCONF_PROFILE, GDM_DCONF_PROFILE_PATH, GDM_FACE_OVERRIDE_PATH,
    KDE_FACE_PAM_FILE, KDE_SMARTCARD_PAM_FILE, PLASMALOGIN_FACE_PAM_FILE,
};
use gaze_security::tpm::TPM_DEVICES;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

mod camera;
mod config;
mod daemon;
mod desktop;
mod gnome;
mod keyring;
mod pam;

use camera::*;
use config::*;
use daemon::*;
use desktop::*;
use gnome::*;
use keyring::*;
use pam::*;

const DAEMON_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(25);
const BENCHMARK_TIMEOUT: Duration = Duration::from_secs(30);
const PAM_MODULES: [&str; 2] = ["pam_gaze.so", "pam_gaze_grosshack.so"];
const GAZE_BUS_NAME: &str = "com.gundulabs.Gaze";
const GNOME_EXTENSION_ID: &str = "gaze@gundulabs.com";
const GNOME_EXTENSION_SCHEMA: &str = "org.gnome.shell.extensions.gaze";
const GNOME_DOCS_URL: &str = "https://gaze.gundulabs.com/guide/gnome";
/// PAM falls back here when `/etc/pam.d` has no such service, and Arch, Debian and
/// openSUSE ship these slots only there, so reading `/etc` alone sees nothing.
const VENDOR_PAM_DIR: &str = "/usr/lib/pam.d";
const POLKIT_PAM_FILE: &str = "/etc/pam.d/polkit-1";
const ELEVATION_PAM_SERVICE: &str = "sudo";
const PAM_SUDO_OPTOUT_PATH: &str = "/etc/gaze/pam-sudo.optout";

fn read_pam_service(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().or_else(|| {
        let name = path.rsplit('/').next()?;
        fs::read_to_string(format!("{VENDOR_PAM_DIR}/{name}")).ok()
    })
}
const GDM_ENABLED_EXTENSIONS_KEY: &str = "/org/gnome/shell/enabled-extensions";
const GDM_DISABLE_EXTENSIONS_KEY: &str = "/org/gnome/shell/disable-user-extensions";
/// Debian and Ubuntu name the account `gdm3`, everyone else `gdm`.
const GDM_HOME_DIRS: [&str; 2] = ["/var/lib/gdm", "/var/lib/gdm3"];
const GDM_COMPILED_DB_PATH: &str = "/etc/dconf/db/gdm";
const GDM_FACE_PAM_SERVICE: &str = "gdm-face";
const GREETD_PAM_FILE: &str = "/etc/pam.d/greetd";
/// Files that decide what runs as root or who may talk to the daemon. A writable entry here
/// is a path to root, so they are held to the same ownership rule as the PAM stack.
const PRIVILEGED_FILES: [&str; 5] = [
    "/usr/lib/systemd/system/gazed.service",
    "/lib/systemd/system/gazed.service",
    "/etc/dbus-1/system.d/com.gundulabs.Gaze.conf",
    "/usr/share/dbus-1/system.d/com.gundulabs.Gaze.conf",
    "/usr/share/polkit-1/actions/com.gundulabs.gaze.policy",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Pass,
    /// A working feature the user deliberately switched off: no checkmark, but it
    /// still carries the steps that switch it on.
    Off,
    Warning,
    Error,
}

#[derive(Debug)]
struct Check {
    level: Level,
    name: &'static str,
    message: String,
    fix: Option<String>,
}

#[derive(Default)]
struct Report {
    checks: Vec<Check>,
}

impl Report {
    fn push(
        &mut self,
        level: Level,
        name: &'static str,
        message: impl Into<String>,
        fix: Option<impl Into<String>>,
    ) {
        self.checks.push(Check {
            level,
            name,
            message: message.into(),
            fix: fix.map(Into::into),
        });
    }

    fn pass(&mut self, name: &'static str, message: impl Into<String>) {
        self.push(Level::Pass, name, message, None::<String>);
    }

    fn off(&mut self, name: &'static str, message: impl Into<String>, how: impl Into<String>) {
        self.push(Level::Off, name, message, Some(how));
    }

    fn warning(&mut self, name: &'static str, message: impl Into<String>, fix: impl Into<String>) {
        self.push(Level::Warning, name, message, Some(fix));
    }

    fn error(&mut self, name: &'static str, message: impl Into<String>, fix: impl Into<String>) {
        self.push(Level::Error, name, message, Some(fix));
    }

    fn count(&self, level: Level) -> usize {
        self.checks
            .iter()
            .filter(|check| check.level == level)
            .count()
    }

    fn is_healthy(&self) -> bool {
        self.count(Level::Error) == 0
    }

    fn print(&self) -> anyhow::Result<()> {
        let term = Term::stdout();
        term.write_line(&format!("\n{}\n", style("Gaze doctor").cyan().bold()))?;

        for check in &self.checks {
            let (symbol, label) = match check.level {
                Level::Pass => (style("✓").green().bold(), style(check.name).bold()),
                Level::Off => (style("○").dim().bold(), style(check.name).dim().bold()),
                Level::Warning => (
                    style("!").yellow().bold(),
                    style(check.name).yellow().bold(),
                ),
                Level::Error => (style("✗").red().bold(), style(check.name).red().bold()),
            };
            term.write_line(&format!("  {symbol} {label}: {}", check.message))?;
            if let Some(fix) = &check.fix {
                for line in fix.lines() {
                    term.write_line(&format!("      {}", style(line).dim()))?;
                }
            }
        }

        let passed = self.count(Level::Pass);
        let off = self.count(Level::Off);
        let warnings = self.count(Level::Warning);
        let errors = self.count(Level::Error);
        term.write_line(&format!(
            "\n{} {passed} passed, {off} off, {warnings} warning{}, {errors} error{}",
            style("Summary:").bold(),
            if warnings == 1 { "" } else { "s" },
            if errors == 1 { "" } else { "s" }
        ))?;
        if off > 0 {
            term.write_line(
                &style("○ marks a working feature you have switched off; the line under it turns it on.")
                    .dim()
                    .to_string(),
            )?;
        }
        term.write_line("")?;
        Ok(())
    }
}

pub async fn run(username: &str, benchmark: bool) -> anyhow::Result<bool> {
    let mut report = Report::default();

    check_platform(&mut report);
    check_systemd(&mut report);
    let config = check_config(&mut report);
    check_pam(&mut report);
    check_sudo_policy(&mut report, username);
    check_privileged_files(&mut report);
    check_desktop_integration(&mut report);
    check_kde_confirmation_bypass(
        &mut report,
        config.as_ref(),
        read_pam_service(KDE_FACE_PAM_FILE).as_deref(),
        read_pam_service(KDE_SMARTCARD_PAM_FILE).as_deref(),
        read_pam_service(PLASMALOGIN_FACE_PAM_FILE).as_deref(),
    );
    check_tpm(&mut report, config.as_ref());
    check_keyring(&mut report, username, config.as_ref());
    check_kwallet(&mut report, username, config.as_ref());
    check_greeter_keyring_selinux(&mut report, config.as_ref());
    check_daemon(&mut report, username, config.as_ref(), benchmark).await;

    report.print()?;
    Ok(report.is_healthy())
}

fn check_platform(report: &mut Report) {
    if std::env::consts::OS != "linux" {
        report.error(
            "Platform",
            format!("{} is not supported", std::env::consts::OS),
            "Run Gaze on Linux.",
        );
        return;
    }

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if gaze_core::cpu::supports_inference() {
            report.pass("CPU", "AVX2 is available");
        } else {
            report.error(
                "CPU",
                gaze_core::cpu::UNSUPPORTED_CPU_MESSAGE,
                gaze_core::cpu::UNSUPPORTED_CPU_FIX,
            );
        }
    }

    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    report.pass(
        "CPU",
        format!(
            "{} does not require the x86 AVX2 check",
            std::env::consts::ARCH
        ),
    );
}

fn command_output(program: &str, args: &[&str]) -> std::io::Result<(bool, String)> {
    command_output_env(program, args, &[])
}

fn command_output_env(
    program: &str,
    args: &[&str],
    env: &[(&str, &OsStr)],
) -> std::io::Result<(bool, String)> {
    let mut command = Command::new(program);
    command.args(args);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr)
    } else {
        String::from_utf8_lossy(&output.stdout)
    };
    Ok((output.status.success(), text.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_privileged_file_list_covers_every_route_to_root() {
        for needle in [
            "systemd/system/gazed.service",
            "dbus-1/system.d/com.gundulabs.Gaze.conf",
            "polkit-1/actions/com.gundulabs.gaze.policy",
        ] {
            assert!(
                PRIVILEGED_FILES.iter().any(|path| path.contains(needle)),
                "{needle} is not checked"
            );
        }
    }

    #[test]
    fn a_feature_switched_off_is_not_a_checkmark_and_still_says_how_to_turn_it_on() {
        let mut report = Report::default();
        report.off("GDM login face auth", "off", "Turn it on: flip the switch.");

        let check = &report.checks[0];
        assert_eq!(check.level, Level::Off);
        assert_ne!(
            check.level,
            Level::Pass,
            "an off feature must not render as a passing check"
        );
        assert!(
            check.fix.is_some(),
            "an off feature always carries the steps that turn it on"
        );
        assert!(
            report.is_healthy(),
            "switching a feature off is a choice, not a failure"
        );
    }

    #[test]
    fn report_health_depends_on_errors_not_warnings() {
        let mut report = Report::default();
        report.pass("test", "ok");
        report.warning("test", "advisory", "fix");
        assert!(report.is_healthy());
        report.error("test", "broken", "fix");
        assert!(!report.is_healthy());
    }
}
