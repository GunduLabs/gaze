use console::{Term, style};
use gaze_core::config::{CONFIG_PATH, Config};
use gaze_core::dbus::{
    GazeProxy, dbus_error_message, dbus_is_file_not_found, dbus_is_not_activatable,
};
use std::collections::BTreeSet;
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
            "\n{} {passed} passed, {warnings} warnings, {errors} errors\n",
            style("Summary:").bold()
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
    let output = Command::new(program).args(args).output()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr)
    } else {
        String::from_utf8_lossy(&output.stdout)
    };
    Ok((output.status.success(), text.trim().to_string()))
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
            report.pass(
                "Configuration",
                format!("{CONFIG_PATH} parses successfully"),
            );
            config
        }
        Err(err)
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|err| err.kind() == std::io::ErrorKind::PermissionDenied) =>
        {
            report.pass(
                "Configuration",
                format!("{CONFIG_PATH} is private; values will be checked through gazed"),
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

    if let Err(err) = config.security.validate() {
        error(
            err.to_string(),
            "Choose a supported security level with `gaze config`.",
        );
    }

    if config.security.level == "custom" {
        if !config.security.threshold.is_finite()
            || !(0.0..=1.0).contains(&config.security.threshold)
        {
            error(
                format!(
                    "security.threshold must be between 0.0 and 1.0, got {}",
                    config.security.threshold
                ),
                "Set a valid custom threshold in /etc/gaze/config.toml.",
            );
        }
        if !matches!(
            config.security.hybrid_policy.as_str(),
            "" | "default" | "or" | "fallback_on_dark" | "and"
        ) {
            error(
                format!(
                    "unsupported security.hybrid_policy {:?}",
                    config.security.hybrid_policy
                ),
                "Use default, or, fallback_on_dark, or and.",
            );
        }
    }

    let rgb = config.cameras.rgb.trim();
    let ir = config.cameras.ir.trim();
    if rgb.is_empty() && ir.is_empty() {
        error(
            "both cameras.rgb and cameras.ir are empty".to_string(),
            "Set cameras.rgb to \"primary\" or configure an IR camera.",
        );
    }
    if rgb.starts_with("/dev/video") {
        error(
            format!("direct RGB camera path {rgb:?} is unsupported"),
            "Use `gaze config` to select a PipeWire camera; direct /dev/video* is supported only for cameras.ir.",
        );
    }

    if config.liveness.enabled {
        if !config.liveness.threshold.is_finite()
            || !(0.0..=1.0).contains(&config.liveness.threshold)
        {
            error(
                format!(
                    "liveness.threshold must be between 0.0 and 1.0, got {}",
                    config.liveness.threshold
                ),
                "Set liveness.threshold to a value between 0.0 and 1.0.",
            );
        }
        if config.liveness.max_frames == 0 {
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
            message: "camera, security, enrollment, and liveness values are valid".to_string(),
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

fn find_pam_modules() -> Vec<PathBuf> {
    pam_search_dirs()
        .into_iter()
        .flat_map(|dir| PAM_MODULES.map(|module| dir.join(module)))
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

fn find_pam_references() -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir("/etc/pam.d") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let contents = fs::read_to_string(&path).ok()?;
            contents.lines().any(pam_line_has_reference).then_some(path)
        })
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
            "pam_gaze.so is not installed in a standard PAM module directory",
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
    [
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default(),
        std::env::var("XDG_SESSION_DESKTOP").unwrap_or_default(),
        std::env::var("DESKTOP_SESSION").unwrap_or_default(),
    ]
    .join(":")
    .to_ascii_lowercase()
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

        match command_output(
            "gsettings",
            &[
                "get",
                "org.gnome.shell.extensions.gaze",
                "enable-face-authentication",
            ],
        ) {
            Ok((true, value)) if value == "true" => {
                report.pass("GNOME face auth", "enabled for the current user");
            }
            Ok((true, _)) => report.warning(
                "GNOME face auth",
                "disabled for the current user",
                "Run `gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true`.",
            ),
            Ok((false, message)) => report.warning(
                "GNOME face auth",
                format!("could not read the extension setting: {message}"),
                "Reinstall the Gaze GNOME extension package.",
            ),
            Err(err) => report.warning(
                "GNOME face auth",
                format!("could not read the extension setting: {err}"),
                "Reinstall the Gaze GNOME extension package.",
            ),
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
            .is_some_and(|contents| {
                contents.lines().any(|line| {
                    let line = line.split('#').next().unwrap_or_default();
                    line.contains("pam_module")
                        && (line.contains("hyprlock-gaze")
                            || line.contains("hyprlock-gaze-simultaneous"))
                })
            });
        if configured {
            report.pass("hyprlock", "configured to use a Gaze PAM service");
        } else {
            report.warning(
                "hyprlock",
                "the current user's hyprlock.conf does not select a Gaze PAM service",
                "Set `pam_module = hyprlock-gaze` in the hyprlock general block.",
            );
        }
    }
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

    if ["/dev/tpmrm0", "/dev/tpm0"]
        .iter()
        .any(|path| Path::new(path).exists())
    {
        report.pass("TPM", "a TPM device is present for encrypted templates");
    } else {
        report.error(
            "TPM",
            "template encryption is enabled but no TPM device is present",
            "Enable TPM 2.0 in firmware or set storage.encrypt_templates = false, then restart gazed.",
        );
    }
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

    let outcome = tokio::time::timeout(BENCHMARK_TIMEOUT, proxy.benchmark()).await;
    let _ = term.clear_last_lines(1);

    match outcome {
        Ok(Ok(results)) => {
            for result in results {
                report.pass(
                    "Benchmark",
                    format!(
                        "{}: {:.1}ms avg ({:.1} fps), {:.1}ms p95, {:.1}ms min",
                        result.component, result.mean_ms, result.fps, result.p95_ms, result.min_ms
                    ),
                );
            }
        }
        Ok(Err(err)) => report.warning(
            "Benchmark",
            format!(
                "gazed could not run the benchmark: {}",
                dbus_error_message(&err)
            ),
            "Restart gazed and inspect its journal.",
        ),
        Err(_) => report.warning(
            "Benchmark",
            "benchmark timed out",
            "Restart gazed and inspect its journal.",
        ),
    }
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
                            "Run `gaze config` and select a currently detected camera.",
                        );
                    }
                } else if !rgb.starts_with("/dev/video") {
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
            Ok(_) => report.error(
                "IR camera",
                format!("configured source is not visible: {ir}"),
                "Run `gaze config` and select a currently detected IR camera.",
            ),
            Err(err) => report.error(
                "IR camera",
                format!("GStreamer IR camera enumeration failed: {err}"),
                "Verify PipeWire is running and the IR device is connected.",
            ),
        }
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
        config.security.threshold = 1.5;
        config.security.hybrid_policy = "sometimes".to_string();
        config.cameras.rgb = "/dev/video0".to_string();
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
                .any(|message| message.contains("security.threshold"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("hybrid_policy"))
        );
        assert!(
            messages
                .iter()
                .any(|message| message.contains("direct RGB"))
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
