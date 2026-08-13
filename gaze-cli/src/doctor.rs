// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use console::{Term, style};
use gaze_core::config::{
    CONFIG_PATH, Config, MAX_LIVENESS_THRESHOLD, MIN_LIVENESS_MAX_FRAMES, MIN_LIVENESS_THRESHOLD,
    SecurityField, unknown_config_keys,
};
use gaze_core::dbus::{
    GazeProxy, dbus_error_message, dbus_is_file_not_found, dbus_is_not_activatable,
    try_benchmark_from_daemon,
};
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const DAEMON_TIMEOUT: Duration = Duration::from_secs(5);
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(25);
const BENCHMARK_TIMEOUT: Duration = Duration::from_secs(30);
const PAM_MODULES: [&str; 2] = ["pam_gaze.so", "pam_gaze_grosshack.so"];
const GNOME_EXTENSION_ID: &str = "gaze@gundulabs.com";
const GNOME_EXTENSION_SCHEMA: &str = "org.gnome.shell.extensions.gaze";
const GDM_FACE_OVERRIDE_PATH: &str = "/etc/dconf/db/gdm.d/99-gaze";
/// Mirror `pam_gaze_core`'s paths for the slots Plasma starts up front.
const KDE_FACE_PAM_FILE: &str = "/etc/pam.d/kde-fingerprint";
const KDE_SMARTCARD_PAM_FILE: &str = "/etc/pam.d/kde-smartcard";
const PLASMALOGIN_FACE_PAM_FILE: &str = "/etc/pam.d/plasmalogin-fingerprint";
/// PAM falls back here when `/etc/pam.d` has no such service, and Arch, Debian and
/// openSUSE ship these slots only there, so reading `/etc` alone sees nothing.
const VENDOR_PAM_DIR: &str = "/usr/lib/pam.d";

fn read_pam_service(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().or_else(|| {
        let name = path.rsplit('/').next()?;
        fs::read_to_string(format!("{VENDOR_PAM_DIR}/{name}")).ok()
    })
}
const GDM_DCONF_PROFILE: &str = "gdm";
const GDM_DCONF_PROFILE_PATH: &str = "/etc/dconf/profile/gdm";
const GDM_DCONF_FACE_AUTH_KEY: &str = "/org/gnome/shell/extensions/gaze/enable-face-authentication";
const GDM_ENABLED_EXTENSIONS_KEY: &str = "/org/gnome/shell/enabled-extensions";
const GDM_COMPILED_DB_PATH: &str = "/etc/dconf/db/gdm";
const GDM_FACE_PAM_SERVICE: &str = "gdm-face";
const TPM_DEVICES: [&str; 2] = ["/dev/tpmrm0", "/dev/tpm0"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Level {
    Pass,
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
                Level::Warning => (
                    style("!").yellow().bold(),
                    style(check.name).yellow().bold(),
                ),
                Level::Error => (style("✗").red().bold(), style(check.name).red().bold()),
            };
            term.write_line(&format!("  {symbol} {label}: {}", check.message))?;
            if let Some(fix) = &check.fix {
                term.write_line(&format!("      {}", style(fix).dim()))?;
            }
        }

        let passed = self.count(Level::Pass);
        let warnings = self.count(Level::Warning);
        let errors = self.count(Level::Error);
        term.write_line(&format!(
            "\n{} {passed} passed, {warnings} warning{}, {errors} error{}\n",
            style("Summary:").bold(),
            if warnings == 1 { "" } else { "s" },
            if errors == 1 { "" } else { "s" }
        ))?;
        Ok(())
    }
}

pub async fn run(username: &str, benchmark: bool) -> anyhow::Result<bool> {
    let mut report = Report::default();

    check_platform(&mut report);
    check_systemd(&mut report);
    let config = check_config(&mut report);
    check_pam(&mut report);
    check_desktop_integration(&mut report);
    check_tpm(&mut report, config.as_ref());
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
        if std::arch::is_x86_feature_detected!("avx2") {
            report.pass("CPU", "AVX2 is available");
        } else {
            report.error(
                "CPU",
                "AVX2 is unavailable; gazed cannot run on this CPU",
                "Use a machine with AVX2 support. The CLI can run here, but the daemon cannot.",
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

fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    match std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
        Some(home) => dirs.push(PathBuf::from(home)),
        None => {
            if let Some(home) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(home).join(".local/share"));
            }
        }
    }
    match std::env::var_os("XDG_DATA_DIRS").filter(|value| !value.is_empty()) {
        Some(value) => dirs.extend(std::env::split_paths(&value)),
        None => dirs.extend([
            PathBuf::from("/usr/local/share"),
            PathBuf::from("/usr/share"),
        ]),
    }
    dirs
}

// Nix installs the extension's schema inside the extension directory rather
// than into the system schema path, so `gsettings` cannot see it unaided.
fn extension_schema_dir() -> Option<PathBuf> {
    extension_schema_dir_in(&xdg_data_dirs())
}

fn extension_schema_dir_in(data_dirs: &[PathBuf]) -> Option<PathBuf> {
    data_dirs
        .iter()
        .map(|dir| {
            dir.join("gnome-shell")
                .join("extensions")
                .join(GNOME_EXTENSION_ID)
                .join("schemas")
        })
        .find(|dir| dir.join("gschemas.compiled").exists())
}

fn extension_setting(key: &str) -> std::io::Result<(bool, String)> {
    let schema_dir = extension_schema_dir();
    let env: Vec<(&str, &OsStr)> = schema_dir
        .as_deref()
        .map(|dir| vec![("GSETTINGS_SCHEMA_DIR", dir.as_os_str())])
        .unwrap_or_default();
    command_output_env("gsettings", &["get", GNOME_EXTENSION_SCHEMA, key], &env)
}

fn gdm_system_dconf_read(key: &str) -> Option<String> {
    let profile = std::env::temp_dir().join(format!(
        "gaze-doctor-{}-{}.profile",
        GDM_DCONF_PROFILE,
        std::process::id()
    ));
    fs::write(&profile, format!("system-db:{GDM_DCONF_PROFILE}\n")).ok()?;
    let result = command_output_env(
        "dconf",
        &["read", key],
        &[("DCONF_PROFILE", profile.as_os_str())],
    );
    let _ = fs::remove_file(&profile);
    match result {
        Ok((true, value)) => Some(value.trim().to_string()),
        _ => None,
    }
}

/// The value GDM itself sees, which a NixOS configuration sets without `GDM_FACE_OVERRIDE_PATH`.
fn gdm_face_auth_from_dconf() -> Option<bool> {
    if !Path::new(GDM_DCONF_PROFILE_PATH).exists() {
        return None;
    }
    match gdm_system_dconf_read(GDM_DCONF_FACE_AUTH_KEY)?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn profile_reads_system_db(contents: &str, db: &str) -> bool {
    let wanted = format!("system-db:{db}");
    contents.lines().any(|line| line.trim() == wanted)
}

fn extensions_include(value: &str, uuid: &str) -> bool {
    value
        .trim_matches(|c: char| c == '[' || c == ']')
        .split(',')
        .any(|entry| entry.trim().trim_matches(|c| c == '\'' || c == '"') == uuid)
}

enum GdmGreeterReadiness {
    Ready,
    ProfileMissingSystemDb,
    CompiledDbMissing,
    ExtensionNotEnabled,
    Unverifiable(String),
}

fn gdm_greeter_readiness() -> GdmGreeterReadiness {
    match fs::read_to_string(GDM_DCONF_PROFILE_PATH) {
        Ok(contents) if !profile_reads_system_db(&contents, GDM_DCONF_PROFILE) => {
            return GdmGreeterReadiness::ProfileMissingSystemDb;
        }
        Ok(_) => {}
        Err(err) => {
            return GdmGreeterReadiness::Unverifiable(format!(
                "could not read {GDM_DCONF_PROFILE_PATH}: {err}"
            ));
        }
    }

    if !Path::new(GDM_COMPILED_DB_PATH).exists() {
        return GdmGreeterReadiness::CompiledDbMissing;
    }

    match gdm_system_dconf_read(GDM_ENABLED_EXTENSIONS_KEY) {
        Some(value) if extensions_include(&value, GNOME_EXTENSION_ID) => GdmGreeterReadiness::Ready,
        Some(_) => GdmGreeterReadiness::ExtensionNotEnabled,
        None => GdmGreeterReadiness::Unverifiable(
            "`dconf read` against the GDM database failed".to_string(),
        ),
    }
}

fn check_systemd(report: &mut Report) {
    if !Path::new("/run/systemd/system").exists() {
        report.warning(
            "systemd",
            "systemd is not running, so the gazed service state could not be checked",
            "On a normal installation, boot with systemd and run `systemctl status gazed`.",
        );
        return;
    }

    match command_output("systemctl", &["is-active", "gazed"]) {
        Ok((true, state)) if state == "active" => report.pass("Service", "gazed is active"),
        Ok((_, state)) => report.error(
            "Service",
            format!("gazed is {state}"),
            "Run `sudo systemctl enable --now gazed`, then inspect `journalctl -u gazed -n 100 --no-pager` if it fails.",
        ),
        Err(err) => report.error(
            "Service",
            format!("could not query gazed: {err}"),
            "Run `systemctl status gazed`.",
        ),
    }

    match command_output("systemctl", &["is-enabled", "gazed"]) {
        Ok((true, state)) if state == "enabled" => {
            report.pass("Autostart", "gazed is enabled at boot");
        }
        Ok((_, state)) => report.warning(
            "Autostart",
            format!("gazed is {state}"),
            "Run `sudo systemctl enable gazed` so authentication still works after reboot.",
        ),
        Err(err) => report.warning(
            "Autostart",
            format!("could not query gazed enablement: {err}"),
            "Run `systemctl is-enabled gazed`.",
        ),
    }
}

fn check_config(report: &mut Report) -> Option<Config> {
    let path = Path::new(CONFIG_PATH);
    if !path.exists() {
        report.error(
            "Configuration",
            format!("{CONFIG_PATH} does not exist"),
            "Reinstall Gaze or restore the packaged config file.",
        );
        return None;
    }

    check_config_permissions(report, path);

    let config = match Config::load_from(CONFIG_PATH) {
        Ok(config) => {
            let unknown = fs::read_to_string(CONFIG_PATH)
                .map(|contents| unknown_config_keys(&contents))
                .unwrap_or_default();
            if unknown.is_empty() {
                report.pass(
                    "Configuration",
                    format!("{CONFIG_PATH} parses successfully"),
                );
            } else {
                report.warning(
                    "Configuration",
                    format!(
                        "{CONFIG_PATH} parses, but Gaze does not read: {}",
                        unknown.join(", ")
                    ),
                    "Remove or correct those keys; they have no effect, so a misspelled setting is silently off.",
                );
            }
            config
        }
        Err(err)
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|err| err.kind() == std::io::ErrorKind::PermissionDenied) =>
        {
            report.pass(
                "Configuration",
                format!(
                    "{CONFIG_PATH} is not readable here; values are checked through gazed, but the file itself is not inspected for unknown keys"
                ),
            );
            return None;
        }
        Err(err) => {
            report.error(
                "Configuration",
                format!("could not load {CONFIG_PATH}: {err}"),
                "Check the file and fix its TOML syntax, then run `sudo systemctl restart gazed`.",
            );
            return None;
        }
    };

    for check in config_findings(&config) {
        report.checks.push(check);
    }

    Some(config)
}

fn check_config_permissions(report: &mut Report, path: &Path) {
    match fs::metadata(path) {
        Ok(metadata) => {
            let mode = metadata.mode() & 0o777;
            if metadata.uid() != 0 {
                report.error(
                    "Config ownership",
                    format!("{CONFIG_PATH} is owned by UID {}", metadata.uid()),
                    format!("Run `sudo chown root:root {CONFIG_PATH}`."),
                );
            } else if mode & 0o022 != 0 {
                report.error(
                    "Config permissions",
                    format!("{CONFIG_PATH} has writable mode {mode:o}"),
                    format!("Run `sudo chmod 0644 {CONFIG_PATH}`."),
                );
            } else {
                report.pass(
                    "Config permissions",
                    format!("root-owned and not writable by group or others ({mode:o})"),
                );
            }
        }
        Err(err) => report.error(
            "Config permissions",
            format!("could not inspect {CONFIG_PATH}: {err}"),
            format!("Run `sudo stat {CONFIG_PATH}`."),
        ),
    }
}

fn config_findings(config: &Config) -> Vec<Check> {
    let mut findings = Vec::new();
    let mut error = |message: String, fix: &'static str| {
        findings.push(Check {
            level: Level::Error,
            name: "Config values",
            message,
            fix: Some(fix.to_string()),
        });
    };

    for err in config.security.validation_errors() {
        let fix = match err.field {
            SecurityField::Level | SecurityField::ModelQuality => {
                "Choose a supported security level with `gaze config`."
            }
            SecurityField::Threshold => {
                "Set valid custom RGB and IR thresholds in /etc/gaze/config.toml."
            }
            SecurityField::HybridPolicy => "Use default, or, fallback_on_dark, or and.",
        };
        error(err.message, fix);
    }
    if let Err(err) = config.enrollment.validate() {
        error(
            err.to_string(),
            "Set enrollment.min_face_size_ratio to a value from 0.10 through 0.75.",
        );
    }
    if let Err(err) = config.inference.validate() {
        let fix = if cfg!(feature = "openvino") {
            "Use cpu/cpu, openvino/cpu, openvino/gpu, or openvino/npu in the [inference] table."
        } else {
            "Use cpu/cpu, or install a Gaze build compiled with the openvino Cargo feature."
        };
        error(err.to_string(), fix);
    }

    let rgb = config.cameras.rgb.trim();
    let ir = config.cameras.ir.trim();
    if rgb.is_empty() && ir.is_empty() {
        error(
            "both cameras.rgb and cameras.ir are empty".to_string(),
            "Set cameras.rgb to \"primary\" or configure an IR camera.",
        );
    }
    if let Some(index) = rgb.strip_prefix("/dev/video") {
        if index.is_empty() || !index.chars().all(|c| c.is_ascii_digit()) {
            error(
                format!("invalid RGB camera node {rgb:?}"),
                "Use /dev/video<number>, usb:VVVV:PPPP, \"primary\", or a GStreamer source.",
            );
        }
    } else if rgb.starts_with("usb:") && gaze_core::camera::parse_usb_spec(rgb).is_none() {
        error(
            format!("invalid RGB USB spec {rgb:?}"),
            "Use usb:VVVV:PPPP with hex VID:PID, for example usb:046d:085e.",
        );
    }

    if config.liveness.enabled {
        if !config.liveness.threshold.is_finite()
            || !(MIN_LIVENESS_THRESHOLD..=MAX_LIVENESS_THRESHOLD)
                .contains(&config.liveness.threshold)
        {
            error(
                format!(
                    "liveness.threshold must be between {MIN_LIVENESS_THRESHOLD} and {MAX_LIVENESS_THRESHOLD}, got {}",
                    config.liveness.threshold
                ),
                "Set liveness.threshold to a value between 0.10 and 1.0.",
            );
        }
        if config.liveness.max_frames < MIN_LIVENESS_MAX_FRAMES {
            error(
                "liveness.max_frames is zero".to_string(),
                "Set liveness.max_frames to a positive value (the default is 40).",
            );
        }
    }

    if findings.is_empty() {
        findings.push(Check {
            level: Level::Pass,
            name: "Config values",
            message: "camera, security, enrollment, inference, and liveness values are valid"
                .to_string(),
            fix: None,
        });
    }

    if config.cameras.emitter_enabled && ir.is_empty() {
        findings.push(Check {
            level: Level::Warning,
            name: "IR emitter",
            message: "cameras.emitter_enabled is true but cameras.ir is empty".to_string(),
            fix: Some("Configure cameras.ir or disable emitter_enabled.".to_string()),
        });
    }
    if !config.liveness.enabled {
        findings.push(Check {
            level: Level::Warning,
            name: "Liveness",
            message: "anti-spoofing is disabled".to_string(),
            fix: Some(
                "Enable [liveness] unless you intentionally accept photo/screen spoofing risk."
                    .to_string(),
            ),
        });
    }
    if config.enrollment.max_templates == 0 {
        findings.push(Check {
            level: Level::Warning,
            name: "Enrollment limit",
            message: "max_templates is zero, which disables template eviction".to_string(),
            fix: Some(
                "Set enrollment.max_templates to a positive value (the default is 2).".into(),
            ),
        });
    }

    findings
}

fn pam_search_dirs() -> BTreeSet<PathBuf> {
    let mut dirs = BTreeSet::from([
        PathBuf::from("/lib/security"),
        PathBuf::from("/lib64/security"),
        PathBuf::from("/usr/lib/security"),
        PathBuf::from("/usr/lib64/security"),
    ]);

    for base in ["/lib", "/usr/lib"] {
        let Ok(entries) = fs::read_dir(base) else {
            continue;
        };
        for entry in entries.flatten() {
            let security = entry.path().join("security");
            if security.is_dir() {
                dirs.insert(security);
            }
        }
    }
    dirs
}

/// Distributions that load the modules from an absolute path, such as NixOS
/// pointing at the store, never populate a system module directory.
fn find_pam_modules() -> BTreeSet<PathBuf> {
    pam_search_dirs()
        .into_iter()
        .flat_map(|dir| PAM_MODULES.map(|module| dir.join(module)))
        .chain(pam_files().iter().flat_map(|(_, contents)| {
            contents
                .lines()
                .flat_map(pam_line_module_paths)
                .collect::<Vec<_>>()
        }))
        .filter(|path| path.exists())
        .collect()
}

fn pam_line_has_reference(line: &str) -> bool {
    let line = line.split('#').next().unwrap_or_default().trim();
    if line.is_empty() {
        return false;
    }
    line.split_ascii_whitespace().any(|token| {
        PAM_MODULES
            .iter()
            .any(|module| token == *module || token.ends_with(&format!("/{module}")))
    })
}

fn pam_line_module_paths(line: &str) -> Vec<PathBuf> {
    line.split('#')
        .next()
        .unwrap_or_default()
        .split_ascii_whitespace()
        .filter(|token| {
            token.starts_with('/')
                && PAM_MODULES
                    .iter()
                    .any(|module| token.ends_with(&format!("/{module}")))
        })
        .map(PathBuf::from)
        .collect()
}

fn pam_files() -> Vec<(PathBuf, String)> {
    let Ok(entries) = fs::read_dir("/etc/pam.d") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let contents = fs::read_to_string(&path).ok()?;
            Some((path, contents))
        })
        .collect()
}

fn find_pam_references() -> Vec<PathBuf> {
    pam_files()
        .into_iter()
        .filter_map(|(path, contents)| contents.lines().any(pam_line_has_reference).then_some(path))
        .collect()
}

const PAM_ORDERING_COMPETITORS: [&str; 2] = ["pam_unix.so", "pam_fprintd.so"];

/// Returns competing auth modules (password, fingerprint) that appear earlier
/// in the `auth` stack than Gaze, which stalls face auth behind their prompts.
fn find_pam_ordering_conflicts(contents: &str) -> Vec<&'static str> {
    let auth_lines: Vec<&str> = contents
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| line.split_ascii_whitespace().next() == Some("auth"))
        .collect();

    let Some(gaze_idx) = auth_lines
        .iter()
        .position(|line| pam_line_has_reference(line))
    else {
        return Vec::new();
    };

    PAM_ORDERING_COMPETITORS
        .into_iter()
        .filter(|module| {
            auth_lines[..gaze_idx]
                .iter()
                .any(|line| line.contains(module))
        })
        .collect()
}

fn check_pam(report: &mut Report) {
    let modules = find_pam_modules();
    let sequential = modules
        .iter()
        .any(|path| path.file_name().is_some_and(|name| name == PAM_MODULES[0]));
    let simultaneous = modules
        .iter()
        .any(|path| path.file_name().is_some_and(|name| name == PAM_MODULES[1]));

    if sequential && simultaneous {
        report.pass("PAM modules", "both modules are installed");
    } else if !sequential {
        report.error(
            "PAM modules",
            "pam_gaze.so is not installed where PAM can load it",
            "Reinstall the base Gaze package before enabling PAM authentication.",
        );
    } else {
        report.warning(
            "PAM modules",
            "pam_gaze_grosshack.so is missing",
            "Reinstall the base Gaze package if you use simultaneous face/password prompts.",
        );
    }

    let insecure: Vec<String> = modules
        .iter()
        .filter_map(|path| {
            let metadata = fs::metadata(path).ok()?;
            (metadata.uid() != 0 || metadata.mode() & 0o022 != 0)
                .then(|| path.display().to_string())
        })
        .collect();
    if !modules.is_empty() {
        if insecure.is_empty() {
            report.pass(
                "PAM permissions",
                "installed modules are root-owned and not writable by group or others",
            );
        } else {
            report.error(
                "PAM permissions",
                format!(
                    "unsafe ownership or write permissions: {}",
                    insecure.join(", ")
                ),
                "Restore these files from the package manager; do not use writable PAM modules.",
            );
        }
    }

    let references = find_pam_references();
    if references.is_empty() {
        report.warning(
            "PAM stack",
            "no active /etc/pam.d file references a Gaze module",
            "Follow the PAM guide for your distribution if you want login, sudo, or lock-screen authentication.",
        );
    } else {
        let names = references
            .iter()
            .filter_map(|path| path.file_name())
            .map(|name| name.to_string_lossy())
            .collect::<Vec<_>>()
            .join(", ");
        report.pass("PAM stack", format!("Gaze is referenced by: {names}"));

        for path in &references {
            let Ok(contents) = fs::read_to_string(path) else {
                continue;
            };
            let conflicts = find_pam_ordering_conflicts(&contents);
            if !conflicts.is_empty() {
                report.warning(
                    "PAM ordering",
                    format!(
                        "{} runs after {} in {}, so face auth won't be tried until those prompts resolve",
                        PAM_MODULES.join("/"),
                        conflicts.join(", "),
                        path.display()
                    ),
                    "Re-run `sudo pam-auth-update --package` (Debian/Ubuntu) or move the Gaze line above pam_unix.so/pam_fprintd.so.",
                );
            }
        }
    }
}

fn desktop_name() -> String {
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

/// The user whose session is being checked: the invoking user under `sudo`.
fn owning_uid() -> u32 {
    std::env::var("SUDO_UID")
        .ok()
        .and_then(|uid| uid.parse().ok())
        .unwrap_or_else(|| unsafe { libc::getuid() })
}

/// Names only, joined like the environment variables above so the callers'
/// `contains` checks are unchanged. `pam-gaze-core` does the same scan, but the
/// CLI does not link it, and pulling in libpam for one lookup is not worth it.
fn desktop_from_processes(uid: u32) -> String {
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

fn check_desktop_integration(report: &mut Report) {
    let desktop = desktop_name();
    if desktop.contains("gnome") {
        match command_output("gnome-extensions", &["list", "--enabled"]) {
            Ok((true, output)) if output.lines().any(|line| line.trim() == GNOME_EXTENSION_ID) => {
                report.pass("GNOME extension", "enabled for the current user");
            }
            Ok((true, _)) => report.warning(
                "GNOME extension",
                "not enabled for the current user",
                format!("Run `gnome-extensions enable {GNOME_EXTENSION_ID}` after logging into GNOME again."),
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
            Ok((true, _)) => report.pass(
                "GNOME lock-screen face auth",
                "disabled for the current user; enable it in the Gaze extension preferences if you want lock-screen face auth",
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
        match (gdm_face_auth_from_dconf(), override_exists) {
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
                        "enabled system-wide via {GDM_FACE_OVERRIDE_PATH}; toggle it from the extension preferences \"GDM login screen\" section"
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
                GdmGreeterReadiness::Unverifiable(why) => report.warning(
                    "GDM login face auth",
                    format!(
                        "{GDM_FACE_OVERRIDE_PATH} enables it, but the greeter configuration could not be verified: {why}"
                    ),
                    "Install the `dconf` command-line tool and re-run `gaze doctor`.",
                ),
            },
            (_, false) => report.pass("GDM login face auth", "disabled"),
        }
    }

    if desktop.contains("hyprland") {
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
enum KdeLockStatus {
    Wired,
    /// The simultaneous module, which deadlocks on this slot.
    Grosshack,
    NotWired,
    /// KScreenLocker has no biometric slot configured at all.
    NoService,
}

fn slot_status(slot: Option<&str>) -> KdeLockStatus {
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
    if auth_lines().any(|line| line.contains("pam_gaze_grosshack.so")) {
        return KdeLockStatus::Grosshack;
    }
    if auth_lines().any(pam_line_has_reference) {
        return KdeLockStatus::Wired;
    }
    KdeLockStatus::NotWired
}

/// Either slot the greeter starts up front will do, so report on whichever has
/// the most to say: one being wired is a pass however the other looks.
fn kde_lock_status(
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

fn check_kde_lock_screen(
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
            format!("{slot} runs pam_gaze_grosshack.so, which waits for a password prompt that KScreenLocker can never answer"),
            format!("Use the plain module there: replace it with `-auth [success=done default=ignore] pam_gaze.so` in {file}, or reinstall gaze-kde."),
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

/// The greeter only scans before you type where it starts a service of its own,
/// which upstream added in plasma-login-manager!185. Everywhere else face auth
/// waits for the submit, exactly as a fingerprint reader does on that screen.
fn check_kde_login_greeter(report: &mut Report, plasmalogin_face: Option<&str>) {
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

fn hyprlock_selects_gaze(contents: &str) -> bool {
    contents.lines().any(|line| {
        let line = line.split('#').next().unwrap_or_default();
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        matches!(key.trim(), "module" | "pam_module") && value.trim().starts_with("hyprlock-gaze")
    })
}

fn check_tpm(report: &mut Report, config: Option<&Config>) {
    let Some(config) = config else {
        return;
    };
    if !config.storage.encrypt_templates {
        report.pass(
            "TPM",
            "template encryption is disabled, so no TPM is required",
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DaemonCredentials {
    uid: u32,
    gids: Vec<u32>,
    dac_override: bool,
}

fn daemon_credentials() -> Option<DaemonCredentials> {
    let (ok, text) = command_output(
        "systemctl",
        &[
            "show",
            "gazed",
            "-p",
            "LoadState",
            "-p",
            "User",
            "-p",
            "SupplementaryGroups",
            "-p",
            "CapabilityBoundingSet",
        ],
    )
    .ok()?;
    if !ok {
        return None;
    }
    parse_daemon_credentials(&text)
}

fn parse_daemon_credentials(text: &str) -> Option<DaemonCredentials> {
    let mut load_state = "";
    let mut user = "";
    let mut groups = "";
    let mut capabilities = "";
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "LoadState" => load_state = value.trim(),
            "User" => user = value.trim(),
            "SupplementaryGroups" => groups = value.trim(),
            "CapabilityBoundingSet" => capabilities = value.trim(),
            _ => {}
        }
    }

    if load_state != "loaded" {
        return None;
    }
    if !(user.is_empty() || user == "root" || user == "0") {
        return None;
    }

    let mut gids = vec![0];
    gids.extend(groups.split_whitespace().filter_map(group_gid));
    Some(DaemonCredentials {
        uid: 0,
        gids,
        dac_override: capabilities
            .split_whitespace()
            .any(|capability| capability.eq_ignore_ascii_case("cap_dac_override")),
    })
}

fn node_openable(uid: u32, gid: u32, mode: u32, credentials: &DaemonCredentials) -> bool {
    if credentials.dac_override {
        return true;
    }
    let class = if uid == credentials.uid {
        0o600
    } else if credentials.gids.contains(&gid) {
        0o060
    } else {
        0o006
    };
    mode & class == class
}

fn group_gid(name: &str) -> Option<u32> {
    let name = std::ffi::CString::new(name).ok()?;
    let entry = unsafe { libc::getgrnam(name.as_ptr()) };
    if entry.is_null() {
        return None;
    }
    Some(unsafe { (*entry).gr_gid })
}

fn user_name(uid: u32) -> String {
    let entry = unsafe { libc::getpwuid(uid) };
    if entry.is_null() {
        return uid.to_string();
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*entry).pw_name) };
    name.to_str().map(str::to_owned).unwrap_or(uid.to_string())
}

fn group_name(gid: u32) -> String {
    let entry = unsafe { libc::getgrgid(gid) };
    if entry.is_null() {
        return gid.to_string();
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*entry).gr_name) };
    name.to_str().map(str::to_owned).unwrap_or(gid.to_string())
}

async fn read_daemon_config(proxy: &GazeProxy<'_>, ready_wait: Duration) -> zbus::Result<Config> {
    let deadline = Instant::now() + ready_wait;
    loop {
        match proxy.config().await {
            Ok(config) => return Ok(config),
            Err(err) if dbus_is_not_activatable(&err) && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

async fn check_daemon(
    report: &mut Report,
    username: &str,
    config: Option<&Config>,
    benchmark: bool,
) {
    let daemon_starting = matches!(
        command_output("systemctl", &["is-active", "gazed"]),
        Ok((true, ref state)) if state == "active"
    );
    let ready_wait = if daemon_starting {
        DAEMON_READY_TIMEOUT
    } else {
        Duration::ZERO
    };

    let proxy = match tokio::time::timeout(DAEMON_TIMEOUT, gaze_core::dbus::connect_gaze()).await {
        Ok(Ok(proxy)) => {
            report.pass("DBus", "com.gundulabs.Gaze is reachable on the system bus");
            proxy
        }
        Ok(Err(err)) => {
            report.error(
                "DBus",
                format!("could not reach com.gundulabs.Gaze: {err}"),
                "Run `systemctl status gazed` and `journalctl -u gazed -n 100 --no-pager`.",
            );
            check_cameras(report, config);
            return;
        }
        Err(_) => {
            report.error(
                "DBus",
                "timed out waiting for com.gundulabs.Gaze",
                "Run `systemctl status gazed` and inspect the daemon journal.",
            );
            check_cameras(report, config);
            return;
        }
    };

    let mut daemon_config = None;
    match tokio::time::timeout(
        ready_wait + DAEMON_TIMEOUT,
        read_daemon_config(&proxy, ready_wait),
    )
    .await
    {
        Ok(Ok(loaded_config)) => {
            report.pass("Daemon", "gazed responded to a configuration request");
            if config.is_none() {
                for check in config_findings(&loaded_config) {
                    report.checks.push(check);
                }
                check_tpm(report, Some(&loaded_config));
            }
            daemon_config = Some(loaded_config);
        }
        Ok(Err(err)) if dbus_is_not_activatable(&err) => report.error(
            "Daemon",
            "gazed is still starting up (models may be downloading)",
            "Wait for the first-run model download to finish, then re-run `gaze doctor`.",
        ),
        Ok(Err(err)) => report.error(
            "Daemon",
            format!(
                "gazed did not return its configuration: {}",
                dbus_error_message(&err)
            ),
            "Restart gazed and inspect its journal.",
        ),
        Err(_) => report.error(
            "Daemon",
            "gazed timed out while reading its configuration",
            "Restart gazed and inspect its journal.",
        ),
    }
    let config = config.or(daemon_config.as_ref());

    match tokio::time::timeout(DAEMON_TIMEOUT, proxy.is_camera_available()).await {
        Ok(Ok(true)) => report.pass(
            "Camera session",
            "the daemon can access the current PipeWire session",
        ),
        Ok(Ok(false)) => report.error(
            "Camera session",
            "the daemon cannot find a usable PipeWire runtime for this session",
            "Run this command from a local graphical session and verify /run/user/$UID/pipewire-0 exists.",
        ),
        Ok(Err(err)) => report.error(
            "Camera session",
            format!("availability check failed: {}", dbus_error_message(&err)),
            "Inspect the gazed journal for PipeWire or login-session errors.",
        ),
        Err(_) => report.error(
            "Camera session",
            "availability check timed out",
            "Restart gazed and inspect its journal.",
        ),
    }
    check_cameras(report, config);

    match tokio::time::timeout(DAEMON_TIMEOUT, proxy.list_faces(username)).await {
        Ok(Ok(faces)) if faces.is_empty() => report.warning(
            "Enrollment",
            format!("no faces are enrolled for {username}"),
            "Run `gaze add-face default`.",
        ),
        Ok(Ok(faces)) => {
            report.pass(
                "Enrollment",
                format!("{} face profile(s) enrolled for {username}", faces.len()),
            );
            if let Some(config) = config {
                let missing_rgb = !config.cameras.rgb.trim().is_empty()
                    && faces.iter().any(|(_, _, has_rgb, _)| !has_rgb);
                let missing_ir = !config.cameras.ir.trim().is_empty()
                    && faces.iter().any(|(_, _, _, has_ir)| !has_ir);
                if missing_rgb || missing_ir {
                    let spectra = match (missing_rgb, missing_ir) {
                        (true, true) => "RGB and IR",
                        (true, false) => "RGB",
                        (false, true) => "IR",
                        (false, false) => unreachable!(),
                    };
                    report.warning(
                        "Enrollment coverage",
                        format!("one or more profiles have no {spectra} captures"),
                        "Run `gaze refine-face <name>` for profiles missing configured camera spectra.",
                    );
                } else {
                    report.pass(
                        "Enrollment coverage",
                        "all profiles cover the configured camera spectra",
                    );
                }
            }
        }
        Ok(Err(err)) if dbus_is_file_not_found(&err) => report.warning(
            "Enrollment",
            format!("no faces are enrolled for {username}"),
            "Run `gaze add-face default`.",
        ),
        Ok(Err(err)) => report.error(
            "Enrollment",
            format!(
                "could not list faces for {username}: {}",
                dbus_error_message(&err)
            ),
            "Run `gaze list-faces` and inspect the daemon journal.",
        ),
        Err(_) => report.error(
            "Enrollment",
            format!("timed out while checking faces for {username}"),
            "Restart gazed and inspect its journal.",
        ),
    }

    if benchmark {
        check_benchmark(report, &proxy).await;
    }
}

async fn check_benchmark(report: &mut Report, proxy: &GazeProxy<'_>) {
    let term = Term::stdout();
    let _ = term.write_line(&format!(
        "{} Benchmarking model inference (this can take a few seconds)...",
        style("i").cyan().bold()
    ));

    let outcome = tokio::time::timeout(BENCHMARK_TIMEOUT, try_benchmark_from_daemon(proxy)).await;
    let _ = term.clear_last_lines(1);

    match outcome {
        Ok(Ok(Some(results))) => {
            for result in results {
                let timings = format!(
                    "{} [{} / {}]: {:.1}ms avg ({:.1} fps), {:.1}ms p95, {:.1}ms min",
                    result.component,
                    result.execution_provider,
                    result.device,
                    result.mean_ms,
                    result.fps,
                    result.p95_ms,
                    result.min_ms
                );
                if result.ran_as_configured() {
                    report.pass("Benchmark", timings);
                } else {
                    report.warning(
                        "Benchmark",
                        format!(
                            "{timings}; configured {}/{} is not in use: {}",
                            result.requested_execution_provider,
                            result.requested_device,
                            if result.fallback_reason.is_empty() {
                                "no reason reported"
                            } else {
                                result.fallback_reason.as_str()
                            }
                        ),
                        "Check the gazed journal for the OpenVINO setup error, or set [inference] back to cpu/cpu.",
                    );
                }
            }
        }
        Ok(Ok(None)) => report.warning(
            "Benchmark",
            "the running daemon reports a benchmark layout this build cannot read",
            "Restart it with `systemctl restart gazed`.",
        ),
        Ok(Err(err)) => report.warning(
            "Benchmark",
            format!("gazed could not run the benchmark: {err}"),
            "Restart gazed and inspect its journal.",
        ),
        Err(_) => report.warning(
            "Benchmark",
            "benchmark timed out",
            "Restart gazed and inspect its journal.",
        ),
    }
}

fn detected_source_remedy(
    cameras: &[(String, String)],
    key: &str,
    automatic: Option<&str>,
) -> String {
    let detected: Vec<&str> = cameras
        .iter()
        .filter(|(_, target)| target != gaze_core::config::DEFAULT_RGB_CAMERA)
        .map(|(_, target)| target.as_str())
        .collect();

    if detected.is_empty() {
        return match automatic {
            Some(automatic) => format!(
                "No PipeWire source is currently advertised. Reconnect the camera, then set \
                 {key} to a detected source, or to \"{automatic}\" to resolve it at runtime."
            ),
            None => format!(
                "No PipeWire source is currently advertised. Reconnect the camera, then set \
                 {key} to a detected source."
            ),
        };
    }

    format!(
        "Run `gaze config` to pick one interactively, or set {key} to one of the detected \
         sources: {}",
        detected
            .iter()
            .map(|target| format!("\"{target}\""))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn check_cameras(report: &mut Report, config: Option<&Config>) {
    let Some(config) = config else {
        return;
    };

    let rgb = config.cameras.rgb.trim();
    if !rgb.is_empty() {
        match gaze_core::camera::enumerate_cameras() {
            Ok(cameras) => {
                let detected = cameras
                    .iter()
                    .filter(|(_, target)| target != gaze_core::config::DEFAULT_RGB_CAMERA)
                    .count();
                if rgb == gaze_core::config::DEFAULT_RGB_CAMERA {
                    if detected > 0 {
                        report.pass(
                            "RGB camera",
                            format!("{detected} color camera(s) visible through PipeWire"),
                        );
                    } else {
                        report.warning(
                            "RGB camera",
                            "no physical color camera was advertised by PipeWire",
                            "Check camera privacy controls and run `gaze config` from the local desktop session.",
                        );
                    }
                } else if rgb.starts_with("pipewiresrc target-object=") {
                    if cameras.iter().any(|(_, target)| target == rgb) {
                        report.pass("RGB camera", "the configured PipeWire source is visible");
                    } else {
                        report.error(
                            "RGB camera",
                            format!("configured source is not visible: {rgb}"),
                            detected_source_remedy(
                                &cameras,
                                "cameras.rgb",
                                Some(gaze_core::config::DEFAULT_RGB_CAMERA),
                            ),
                        );
                    }
                } else if let Some((vid, pid)) = gaze_core::camera::parse_usb_spec(rgb) {
                    report.pass(
                        "RGB camera",
                        format!("resolves the color node for USB {vid:04x}:{pid:04x} at runtime"),
                    );
                } else if rgb.starts_with("/dev/video") {
                    match fs::metadata(rgb) {
                        Ok(metadata) if metadata.file_type().is_char_device() => {
                            report.pass("RGB camera", format!("{rgb} is a character device"));
                        }
                        Ok(_) => report.error(
                            "RGB camera",
                            format!("{rgb} is not a character device"),
                            "Point cameras.rgb at a /dev/video* node.",
                        ),
                        Err(err) => report.error(
                            "RGB camera",
                            format!("{rgb} is not accessible: {err}"),
                            "Check the device path and permissions.",
                        ),
                    }
                } else {
                    report.warning(
                        "RGB camera",
                        "a custom GStreamer source is configured and was not opened by this read-only check",
                        "Run `gaze auth` to verify that the custom source produces frames.",
                    );
                }
            }
            Err(err) => report.error(
                "RGB camera",
                format!("GStreamer camera enumeration failed: {err}"),
                "Verify the GStreamer PipeWire plugin is installed and PipeWire is running.",
            ),
        }
    }

    let ir = config.cameras.ir.trim();
    if ir.is_empty() {
        return;
    }
    if ir.starts_with("/dev/video") {
        match fs::metadata(ir) {
            Ok(metadata) if metadata.file_type().is_char_device() => {
                report.pass("IR camera", format!("{ir} is a character device"));
            }
            Ok(_) => report.error(
                "IR camera",
                format!("{ir} is not a device node"),
                "Choose the IR camera's /dev/video* node.",
            ),
            Err(err) => report.error(
                "IR camera",
                format!("cannot access {ir}: {err}"),
                "Correct cameras.ir or reconnect the IR camera.",
            ),
        }
    } else if ir.starts_with("pipewiresrc target-object=") {
        match gaze_core::camera::enumerate_ir_cameras() {
            Ok(cameras) if cameras.iter().any(|(_, target)| target == ir) => {
                report.pass("IR camera", "the configured PipeWire source is visible");
            }
            Ok(cameras) => report.error(
                "IR camera",
                format!("configured source is not visible: {ir}"),
                detected_source_remedy(&cameras, "cameras.ir", None),
            ),
            Err(err) => report.error(
                "IR camera",
                format!("GStreamer IR camera enumeration failed: {err}"),
                "Verify PipeWire is running and the IR device is connected.",
            ),
        }
    } else if let Some((vid, pid)) = gaze_core::camera::parse_usb_spec(ir) {
        report.pass(
            "IR camera",
            format!("resolves the IR node for USB {vid:04x}:{pid:04x} at runtime"),
        );
    } else {
        report.warning(
            "IR camera",
            "a custom GStreamer source is configured and was not opened by this read-only check",
            "Run `gaze auth` to verify that the IR source produces frames.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gdm_profile_without_the_system_db_is_detected() {
        let debian = "user-db:user\nsystem-db:gdm\nfile-db:/usr/share/gdm/greeter-dconf-defaults\n";
        assert!(profile_reads_system_db(debian, "gdm"));

        for broken in [
            "user-db:user\n",
            "",
            "system-db:distro\nfile-db:/usr/share/gdm/greeter-dconf-defaults\n",
            "system-db:gdmx\n",
            "#system-db:gdm\n",
        ] {
            assert!(
                !profile_reads_system_db(broken, "gdm"),
                "{broken:?} must not count as reading system-db:gdm"
            );
        }
    }

    #[test]
    fn a_greeter_extension_list_is_matched_exactly() {
        let uuid = "gaze@gundulabs.com";
        assert!(extensions_include("['gaze@gundulabs.com']", uuid));
        assert!(extensions_include(
            "['dash-to-dock@micxgx.gmail.com', 'gaze@gundulabs.com']",
            uuid
        ));
        assert!(!extensions_include("@as []", uuid));
        assert!(!extensions_include("['other@example.com']", uuid));
        assert!(
            !extensions_include("['gaze-clock-diag@gundulabs.com']", uuid),
            "a different extension sharing the domain must not match"
        );
    }

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

    fn credentials(gids: &[u32], dac_override: bool) -> DaemonCredentials {
        DaemonCredentials {
            uid: 0,
            gids: gids.to_vec(),
            dac_override,
        }
    }

    #[test]
    fn root_owned_tpm_node_is_openable_without_dac_override() {
        let creds = credentials(&[0], false);
        assert!(node_openable(0, 972, 0o660, &creds));
        assert!(!node_openable(0, 972, 0o060, &creds));
    }

    #[test]
    fn tss_owned_tpm_node_needs_the_group_or_the_capability() {
        let tss_uid = 972;
        let tss_gid = 972;

        let bare_root = credentials(&[0], false);
        assert!(
            !node_openable(tss_uid, tss_gid, 0o660, &bare_root),
            "the reporter's Ubuntu node must read as unopenable"
        );

        assert!(node_openable(
            tss_uid,
            tss_gid,
            0o660,
            &credentials(&[0, tss_gid], false)
        ));
        assert!(node_openable(
            tss_uid,
            tss_gid,
            0o660,
            &credentials(&[0], true)
        ));
        assert!(node_openable(tss_uid, tss_gid, 0o666, &bare_root));
    }

    #[test]
    fn group_access_does_not_rescue_an_owner_class_mismatch() {
        let creds = credentials(&[0, 972], false);
        assert!(!node_openable(0, 972, 0o060, &creds));
    }

    #[test]
    fn daemon_credentials_come_from_the_effective_unit() {
        let parsed = parse_daemon_credentials(
            "LoadState=loaded\nCapabilityBoundingSet=cap_dac_read_search cap_dac_override\nUser=\nSupplementaryGroups=video",
        )
        .expect("a loaded root unit must be understood");
        assert_eq!(parsed.uid, 0);
        assert!(parsed.dac_override);
        assert!(parsed.gids.contains(&0));

        let capless = parse_daemon_credentials(
            "LoadState=loaded\nCapabilityBoundingSet=cap_dac_read_search\nUser=\nSupplementaryGroups=video",
        )
        .expect("a loaded root unit must be understood");
        assert!(!capless.dac_override);

        assert_eq!(
            parse_daemon_credentials("LoadState=not-found\nUser=\nCapabilityBoundingSet="),
            None,
            "an uninstalled unit must not be judged"
        );
        assert_eq!(
            parse_daemon_credentials(
                "LoadState=loaded\nUser=gaze\nCapabilityBoundingSet=cap_dac_read_search"
            ),
            None,
            "a custom User= must not be judged"
        );
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

    #[test]
    fn extension_schema_dir_finds_a_compiled_schema_in_the_extension() {
        let root = std::env::temp_dir().join(format!("gaze-doctor-schema-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let fhs = root.join("usr/share");
        let nix = root.join("nix/share");
        let extension = nix
            .join("gnome-shell")
            .join("extensions")
            .join(GNOME_EXTENSION_ID);
        fs::create_dir_all(fhs.join("gnome-shell/extensions").join(GNOME_EXTENSION_ID)).unwrap();
        fs::create_dir_all(extension.join("schemas")).unwrap();

        let dirs = vec![fhs.clone(), nix.clone()];
        assert_eq!(
            extension_schema_dir_in(&dirs),
            None,
            "an extension directory without a compiled schema must not be used"
        );

        fs::write(extension.join("schemas/gschemas.compiled"), b"").unwrap();
        assert_eq!(
            extension_schema_dir_in(&dirs),
            Some(extension.join("schemas"))
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn pam_line_module_paths_collects_absolute_module_paths() {
        assert_eq!(
            pam_line_module_paths(
                "auth       [success=done default=bad]   /nix/store/abc-gaze-0.2.7/lib/security/pam_gaze.so"
            ),
            vec![PathBuf::from(
                "/nix/store/abc-gaze-0.2.7/lib/security/pam_gaze.so"
            )]
        );
        assert_eq!(
            pam_line_module_paths(
                "auth sufficient /nix/store/abc-gaze/lib/security/pam_gaze_grosshack.so"
            ),
            vec![PathBuf::from(
                "/nix/store/abc-gaze/lib/security/pam_gaze_grosshack.so"
            )]
        );

        assert!(
            pam_line_module_paths("auth sufficient pam_gaze.so").is_empty(),
            "a bare module name resolves through the search directories instead"
        );
        assert!(
            pam_line_module_paths("# auth sufficient /nix/store/abc/lib/security/pam_gaze.so")
                .is_empty(),
            "commented lines are not part of the stack"
        );
        assert!(
            pam_line_module_paths("auth sufficient /usr/lib64/security/pam_fprintd.so").is_empty()
        );
    }

    #[test]
    fn camera_remedy_lists_detected_sources() {
        let cameras = vec![
            (
                "Primary camera".to_string(),
                gaze_core::config::DEFAULT_RGB_CAMERA.to_string(),
            ),
            (
                "Integrated Camera".to_string(),
                "pipewiresrc target-object=v4l2_input.pci-0000_00_14_0".to_string(),
            ),
        ];

        let remedy = detected_source_remedy(
            &cameras,
            "cameras.rgb",
            Some(gaze_core::config::DEFAULT_RGB_CAMERA),
        );
        assert!(remedy.contains("cameras.rgb"), "{remedy}");
        assert!(
            remedy.contains("\"pipewiresrc target-object=v4l2_input.pci-0000_00_14_0\""),
            "the detected source must be quoted verbatim for copy-paste: {remedy}"
        );
        assert!(
            !remedy.contains("\"primary\""),
            "the primary pseudo-source is not a selectable node: {remedy}"
        );
    }

    #[test]
    fn camera_remedy_only_offers_primary_where_it_is_valid() {
        let none_detected = vec![(
            "Primary camera".to_string(),
            gaze_core::config::DEFAULT_RGB_CAMERA.to_string(),
        )];

        let rgb = detected_source_remedy(
            &none_detected,
            "cameras.rgb",
            Some(gaze_core::config::DEFAULT_RGB_CAMERA),
        );
        assert!(rgb.contains("cameras.rgb"), "{rgb}");
        assert!(rgb.contains("\"primary\""), "{rgb}");

        let ir = detected_source_remedy(&[], "cameras.ir", None);
        assert!(ir.contains("cameras.ir"), "{ir}");
        assert!(
            !ir.contains("primary"),
            "cameras.ir has no primary fallback: {ir}"
        );
    }

    #[test]
    fn valid_default_config_has_no_errors() {
        let findings = config_findings(&Config::default());
        assert!(!findings.iter().any(|check| check.level == Level::Error));
    }

    #[test]
    fn config_checks_invalid_thresholds_and_camera_sources() {
        let mut config = Config::default();
        config.security.level = "custom".to_string();
        config.security.detector = "standard".to_string();
        config.security.recognizer = "standard".to_string();
        config.security.rgb_threshold = 1.5;
        config.security.ir_threshold = -0.1;
        config.security.hybrid_policy = "sometimes".to_string();
        config.cameras.rgb = "/dev/videoX".to_string();
        config.enrollment.min_face_size_ratio = 0.05;
        config.liveness.threshold = f64::NAN;
        config.liveness.max_frames = 0;

        let findings = config_findings(&config);
        let messages = findings
            .iter()
            .map(|check| check.message.as_str())
            .collect::<Vec<_>>();
        assert!(
            messages
                .iter()
                .any(|message| message.contains("security.rgb_threshold"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("security.ir_threshold"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("hybrid_policy"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("invalid RGB camera node"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("enrollment.min_face_size_ratio"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("liveness.threshold"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("max_frames"))
        );
    }

    #[test]
    fn every_invalid_security_field_is_reported_exactly_once() {
        let mut config = Config::default();
        config.security.level = "custom".to_string();
        config.security.detector = "standard".to_string();
        config.security.recognizer = "standard".to_string();
        config.security.rgb_threshold = 1.5;
        config.security.ir_threshold = -0.1;
        config.security.hybrid_policy = "sometimes".to_string();

        let findings = config_findings(&config);
        let count = |needle: &str| {
            findings
                .iter()
                .filter(|check| check.message.contains(needle))
                .count()
        };
        assert_eq!(count("security.rgb_threshold"), 1);
        assert_eq!(count("security.ir_threshold"), 1);
        assert_eq!(count("security.hybrid_policy"), 1);
    }

    #[test]
    fn invalid_security_fields_get_their_own_fix() {
        let mut config = Config::default();
        config.security.level = "custom".to_string();
        config.security.detector = "standard".to_string();
        config.security.recognizer = "standard".to_string();
        config.security.rgb_threshold = 1.5;
        config.security.hybrid_policy = "sometimes".to_string();

        let fix_for = |needle: &str| {
            config_findings(&config)
                .into_iter()
                .find(|check| check.message.contains(needle))
                .and_then(|check| check.fix)
                .unwrap_or_default()
        };
        assert!(fix_for("security.rgb_threshold").contains("custom RGB and IR thresholds"));
        assert!(fix_for("security.hybrid_policy").contains("fallback_on_dark"));
    }

    #[test]
    fn pam_reference_parser_ignores_comments_and_accepts_absolute_paths() {
        assert!(!pam_line_has_reference("# auth sufficient pam_gaze.so"));
        assert!(!pam_line_has_reference("auth include system-auth"));
        assert!(pam_line_has_reference("auth sufficient pam_gaze.so"));
        assert!(pam_line_has_reference(
            "auth sufficient /usr/lib/security/pam_gaze_grosshack.so debug"
        ));
        assert!(!pam_line_has_reference(
            "auth sufficient pam_gaze.so.disabled"
        ));
    }

    #[test]
    fn pam_ordering_flags_modules_stacked_before_gaze() {
        let stacked_behind = "auth [success=3 default=ignore] pam_fprintd.so\n\
             auth [success=2 default=ignore] pam_unix.so\n\
             auth [success=1 default=ignore] pam_gaze.so\n";
        assert_eq!(
            find_pam_ordering_conflicts(stacked_behind),
            vec!["pam_unix.so", "pam_fprintd.so"]
        );

        let stacked_first = "auth sufficient pam_gaze.so\n\
             auth sufficient pam_unix.so try_first_pass nullok\n";
        assert!(find_pam_ordering_conflicts(stacked_first).is_empty());

        assert!(find_pam_ordering_conflicts("auth include system-auth\n").is_empty());
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
