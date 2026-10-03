// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) fn check_systemd(report: &mut Report) {
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

pub(super) fn check_config(report: &mut Report) -> Option<Config> {
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

pub(super) fn check_config_permissions(report: &mut Report, path: &Path) {
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

pub(super) fn config_findings(config: &Config) -> Vec<Check> {
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
    if let Err(err) = config.cameras.validate() {
        error(
            err.to_string(),
            "Use never, auto, or always for cameras.parallel_capture.",
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
    } else if rgb.starts_with("usb:") && gaze_vision::camera::parse_usb_spec(rgb).is_none() {
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
        if !config.liveness.max_seconds.is_finite()
            || !(MIN_LIVENESS_MAX_SECONDS..=MAX_LIVENESS_MAX_SECONDS)
                .contains(&config.liveness.max_seconds)
        {
            error(
                format!(
                    "liveness.max_seconds must be between {MIN_LIVENESS_MAX_SECONDS} and {MAX_LIVENESS_MAX_SECONDS}, got {}",
                    config.liveness.max_seconds
                ),
                "Set liveness.max_seconds to a value between 0.2 and 30.0 (the default is 2.0).",
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
    if config.cameras.parallel_capture() == "always" && !ir.is_empty() {
        findings.push(Check {
            level: Level::Warning,
            name: "Parallel capture",
            message: "cameras.parallel_capture is \"always\", which streams RGB and IR at once \
                      without checking that the camera supports it"
                .to_string(),
            fix: Some(
                "If hybrid auth starts failing with \"IR camera stream stopped unexpectedly\", \
                 use \"auto\" or \"never\"."
                    .to_string(),
            ),
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
        config.security.rgb_threshold = 1.5;
        config.security.ir_threshold = -0.1;
        config.security.hybrid_policy = "sometimes".to_string();
        config.cameras.rgb = "/dev/videoX".to_string();
        config.enrollment.min_face_size_ratio = 0.05;
        config.liveness.threshold = f64::NAN;
        config.liveness.max_seconds = 0.0;

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
                .any(|message| message.contains("max_seconds"))
        );
    }

    #[test]
    fn config_checks_the_parallel_capture_mode() {
        let mut config = Config::default();
        config.cameras.parallel_capture = "sometimes".to_string();
        assert!(
            config_findings(&config)
                .iter()
                .any(|check| check.level == Level::Error
                    && check.message.contains("cameras.parallel_capture"))
        );

        let mut forced = Config::default();
        forced.cameras.ir = "/dev/video2".to_string();
        forced.cameras.parallel_capture = "always".to_string();
        assert!(config_findings(&forced).iter().any(
            |check| check.level == Level::Warning && check.message.contains("parallel_capture")
        ));

        let mut detected = Config::default();
        detected.cameras.ir = "/dev/video2".to_string();
        detected.cameras.parallel_capture = "auto".to_string();
        assert!(
            !config_findings(&detected)
                .iter()
                .any(|check| check.message.contains("parallel_capture"))
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
}
