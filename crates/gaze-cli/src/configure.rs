// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use console::{Term, style};
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use gaze_core::config::{
    AuthConfig, CameraSources, Config, DEFAULT_SECURITY_THRESHOLD, HYBRID_POLICY_OPTIONS,
    INFERENCE_DEVICE_OPTIONS, MAX_ENROLLMENT_FACE_SIZE_RATIO, MAX_LIVENESS_MAX_SECONDS,
    MAX_LIVENESS_THRESHOLD, MAX_SECURITY_THRESHOLD, MIN_ENROLLMENT_FACE_SIZE_RATIO,
    MIN_LIVENESS_MAX_SECONDS, MIN_LIVENESS_THRESHOLD, MIN_SECURITY_THRESHOLD,
    MODEL_QUALITY_OPTIONS, START_DELAY_SCOPE_LABELS, SecurityLevel,
};
use gaze_core::dbus::{GazeProxy, apply_config_to_daemon, apply_config_with_keyring_to_daemon};

use crate::{get_current_user, keyring};

pub fn config_show_text(config: &Config) -> anyhow::Result<String> {
    Ok(format!(
        "{}\n# Resolved values\n\
         # security: detector = {}, recognizer = {}, rgb_threshold = {:.2}, ir_threshold = {:.2}, hybrid_policy = {}\n\
         # cameras.parallel_capture = {}\n\
         # auth.start_delay_scope = {}\n",
        config.to_toml()?.trim_end(),
        config.security.detector(),
        config.security.recognizer(),
        config.security.rgb_threshold(),
        config.security.ir_threshold(),
        config.security.hybrid_policy(),
        config.cameras.parallel_capture(),
        config.auth.start_delay_scope()
    ))
}

fn ensure_configured_source_listed(options: &mut Vec<(String, String)>, configured: &str) {
    let configured = configured.trim();
    if configured.is_empty() || gaze_vision::camera::is_listed_source(options, configured) {
        return;
    }
    options.push((format!("{configured} (configured)"), configured.to_string()));
}

fn prompt_security_threshold(
    theme: &ColorfulTheme,
    spectrum: &str,
    default: f64,
) -> anyhow::Result<f64> {
    let value = Input::<String>::with_theme(theme)
        .with_prompt(format!(
            "Custom {spectrum} threshold ({MIN_SECURITY_THRESHOLD} - {MAX_SECURITY_THRESHOLD})"
        ))
        .default(default.to_string())
        .validate_with(|input: &String| match input.trim().parse::<f64>() {
            Ok(value)
                if value.is_finite()
                    && (MIN_SECURITY_THRESHOLD..=MAX_SECURITY_THRESHOLD).contains(&value) =>
            {
                Ok(())
            }
            Ok(_) => Err(format!(
                "must be between {MIN_SECURITY_THRESHOLD} and {MAX_SECURITY_THRESHOLD}"
            )),
            Err(_) => Err("must be a number".to_string()),
        })
        .interact_text()?
        .trim()
        .parse::<f64>()
        .unwrap_or(DEFAULT_SECURITY_THRESHOLD);
    Ok(value)
}

pub fn change_summary(before: &Config, after: &Config) -> anyhow::Result<Vec<String>> {
    fn fields(config: &Config) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
        let mut fields = std::collections::BTreeMap::new();
        let mut section = String::new();
        for line in config.to_toml()?.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line.trim_matches(['[', ']']).to_string();
            } else if let Some((key, value)) = line.split_once('=') {
                fields.insert(
                    format!("{section}.{}", key.trim()),
                    value.trim().to_string(),
                );
            }
        }
        Ok(fields)
    }
    let old = fields(before)?;
    let new = fields(after)?;
    let keys: std::collections::BTreeSet<_> = old.keys().chain(new.keys()).collect();
    Ok(keys
        .into_iter()
        .filter(|key| old.get(*key) != new.get(*key))
        .map(|key| {
            format!(
                "{key}: {} → {}",
                old.get(key).map(String::as_str).unwrap_or("default"),
                new.get(key).map(String::as_str).unwrap_or("default")
            )
        })
        .collect())
}

pub fn print_apply_notes(before: &Config, after: &Config) {
    if before.security.recognizer() != after.security.recognizer() {
        println!(
            "After applying this model change, re-enroll existing profiles with gaze refine-face PROFILE before relying on face unlock. Old captures remain on disk but cannot match with the new recognition model."
        );
    }
    if before.inference.execution_provider != after.inference.execution_provider
        || before.inference.device != after.inference.device
    {
        println!(
            "After applying this acceleration change, run sudo systemctl restart gazed, then gaze doctor --benchmark to check the effective provider; CPU fallback may still be active."
        );
    }
}

pub async fn run_config_wizard(
    term: &Term,
    proxy: &GazeProxy<'_>,
    mut config: Config,
    keyring_supported: gaze_core::dbus::KeyringSupport,
) -> anyhow::Result<()> {
    let theme = ColorfulTheme::default();
    let original = config.clone();
    term.write_line("Gaze settings — choose a section. Changes stay in this draft until you save. Esc returns to the menu; Ctrl+C discards the draft.")?;
    loop {
        let choice = Select::with_theme(&theme)
            .with_prompt("Settings")
            .items([
                "Cameras",
                "Security",
                "Lock screen and password prompts",
                "Advanced settings",
                "Review and save",
                "Discard and exit",
            ])
            .default(0)
            .interact_opt()?;
        let mut draft = config.clone();
        let result = match choice {
            Some(0) => match edit_cameras(term, &theme, &mut draft) {
                Ok(()) => {
                    if Confirm::with_theme(&theme)
                        .with_prompt("Check these cameras now")
                        .default(true)
                        .interact()?
                        && let Err(error) = crate::setup::check_cameras(&draft).await
                    {
                        term.write_line(&format!("Camera check: {error}. Choose Cameras to try another source; nothing has been saved."))?;
                    }
                    Ok(())
                }
                Err(error) if error.is::<BackToMenu>() => Err(error),
                Err(error) => {
                    term.write_line(&format!(
                        "{error}. Draft retained; choose a section to continue."
                    ))?;
                    continue;
                }
            },
            Some(1) => edit_security(term, &theme, &mut draft),
            Some(2) => edit_authentication(term, &theme, &mut draft),
            Some(3) => {
                match Select::with_theme(&theme)
                    .with_prompt("Advanced settings")
                    .items([
                        "Hardware acceleration",
                        "Camera tuning",
                        "Enrollment tuning",
                        "Photo spoof protection",
                        "Encryption and keyrings",
                        "Authentication workarounds",
                        "Back",
                    ])
                    .interact_opt()?
                {
                    Some(0) => edit_acceleration(term, &theme, &mut draft),
                    Some(1) => edit_camera_tuning(term, &theme, &mut draft),
                    Some(2) => edit_enrollment(term, &theme, &mut draft),
                    Some(3) => edit_liveness(term, &theme, &mut draft),
                    Some(4) => edit_storage(term, &theme, &mut draft, keyring_supported),
                    Some(5) => edit_authentication_tuning(term, &theme, &mut draft),
                    _ => Ok(()),
                }
            }
            Some(4) => {
                let changes = change_summary(&original, &config)?;
                if changes.is_empty() {
                    term.write_line("No changes to save.")?;
                    return Ok(());
                }
                for change in changes {
                    term.write_line(&format!("  {change}"))?;
                }
                print_apply_notes(&original, &config);
                if !Confirm::with_theme(&theme)
                    .with_prompt("Apply these changes")
                    .default(false)
                    .interact()?
                {
                    continue;
                }
                let saved = crate::run_busy(
                    "Saving settings",
                    "Preparing models and applying changes. Downloads may take a few minutes…"
                        .into(),
                    crate::tui::Tone::Info,
                    false,
                    async {
                        if keyring_supported.gnome || keyring_supported.kwallet {
                            apply_config_with_keyring_to_daemon(proxy, &config).await
                        } else {
                            apply_config_to_daemon(proxy, &config).await
                        }
                    },
                )
                .await?;
                if let Err(error) = saved {
                    term.write_line(&format!(
                        "Could not save: {error}. Your draft is still here; edit it or retry."
                    ))?;
                    continue;
                }
                term.write_line("✓ Settings saved.")?;
                for (enabled, backend) in [
                    (
                        config.storage.unlock_gnome_keyring,
                        gaze_security::keyring::Backend::Gnome,
                    ),
                    (
                        config.storage.unlock_kwallet,
                        gaze_security::keyring::Backend::KWallet,
                    ),
                ] {
                    if enabled
                        && Confirm::with_theme(&theme)
                            .with_prompt(format!("Set up {} password unlock now", backend.name()))
                            .default(true)
                            .interact()?
                    {
                        enroll_keyring(&config, backend)?;
                    }
                }
                return Ok(());
            }
            _ => {
                term.write_line("Draft discarded. Settings unchanged.")?;
                return Ok(());
            }
        };
        match result {
            Ok(()) => config = draft,
            Err(error) if error.is::<BackToMenu>() => {
                term.write_line("Back to the menu; that section was left as it was.")?;
            }
            Err(error) => {
                term.write_line(&format!("{error}. Your draft has not been saved."))?;
            }
        }
    }
}

#[derive(Debug)]
struct BackToMenu;

impl std::fmt::Display for BackToMenu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("back to the settings menu")
    }
}

impl std::error::Error for BackToMenu {}

trait PickOrBack {
    type Answer;
    fn pick(self) -> anyhow::Result<Self::Answer>;
}

impl PickOrBack for Select<'_> {
    type Answer = usize;
    fn pick(self) -> anyhow::Result<usize> {
        self.interact_opt()?.ok_or_else(|| BackToMenu.into())
    }
}

impl PickOrBack for Confirm<'_> {
    type Answer = bool;
    fn pick(self) -> anyhow::Result<bool> {
        self.interact_opt()?.ok_or_else(|| BackToMenu.into())
    }
}

fn edit_cameras(term: &Term, theme: &ColorfulTheme, config: &mut Config) -> anyhow::Result<()> {
    let _ = term;
    let mut cameras = gaze_vision::camera::rgb_choices()
        .map_err(|e| anyhow::anyhow!("Could not list cameras: {e}"))?;
    ensure_configured_source_listed(&mut cameras, config.cameras.rgb.first());
    let mut default_cam_idx =
        gaze_vision::camera::source_index(&cameras, config.cameras.rgb.first());
    let keep_fallback_idx = if matches!(config.cameras.rgb, CameraSources::Ordered(_)) {
        default_cam_idx = cameras.len();
        cameras.push((
            format!("Keep ordered fallback: {}", config.cameras.rgb),
            String::new(),
        ));
        Some(default_cam_idx)
    } else {
        None
    };
    let cam_names: Vec<String> = cameras.iter().map(|(n, _)| n.clone()).collect();

    let selected_cam_idx = Select::with_theme(theme)
        .with_prompt("Color camera")
        .items(&cam_names)
        .default(default_cam_idx)
        .pick()?;

    if Some(selected_cam_idx) != keep_fallback_idx {
        config.cameras.rgb = cameras[selected_cam_idx].1.clone().into();
    }

    let mut ir_options = gaze_vision::camera::ir_choices();
    ensure_configured_source_listed(&mut ir_options, &config.cameras.ir);

    let ir_names: Vec<String> = ir_options.iter().map(|(n, _)| n.clone()).collect();
    let default_ir_idx = gaze_vision::camera::source_index(&ir_options, &config.cameras.ir);

    let selected_ir_idx = Select::with_theme(theme)
        .with_prompt("Infrared camera (leave None unless you use one)")
        .items(&ir_names)
        .default(default_ir_idx)
        .pick()?;

    config.cameras.ir = ir_options[selected_ir_idx].1.clone();

    if config.cameras.rgb.is_empty() && config.cameras.ir.is_empty() {
        term.write_line(&format!(
            "{} No IR camera selected, so keeping the primary RGB camera",
            style("!").yellow().bold()
        ))?;
        config.cameras.rgb = gaze_core::config::DEFAULT_RGB_CAMERA.into();
    }

    Ok(())
}

fn edit_security(term: &Term, theme: &ColorfulTheme, config: &mut Config) -> anyhow::Result<()> {
    let _ = term;
    let selected = Select::with_theme(theme)
        .with_prompt("Security level")
        .items([
            "Low — faster, easier to match",
            "Medium — recommended",
            "High — stricter; re-enrollment may be needed",
            "Maximum — strictest; re-enrollment may be needed",
            "Custom — advanced tuning",
        ])
        .default(config.security.level_index() as usize)
        .pick()?;

    if let Some(level) = SecurityLevel::preset_from_index(selected) {
        config.security = level;
    } else {
        let seed = config.security.custom_form();

        let selected_det_idx = Select::with_theme(theme)
            .with_prompt("Custom detector level")
            .items(MODEL_QUALITY_OPTIONS)
            .default(SecurityLevel::model_quality_index(&seed.detector) as usize)
            .pick()?;
        let detector = SecurityLevel::model_quality_from_index(selected_det_idx).to_string();

        let selected_rec_idx = Select::with_theme(theme)
            .with_prompt("Custom recognizer level")
            .items(MODEL_QUALITY_OPTIONS)
            .default(SecurityLevel::model_quality_index(&seed.recognizer) as usize)
            .pick()?;
        let recognizer = SecurityLevel::model_quality_from_index(selected_rec_idx).to_string();

        let rgb_threshold = prompt_security_threshold(theme, "RGB", seed.rgb_threshold)?;
        let ir_threshold = prompt_security_threshold(theme, "IR", seed.ir_threshold)?;

        let selected_hybrid_idx = Select::with_theme(theme)
            .with_prompt("Custom hybrid combining policy")
            .items(HYBRID_POLICY_OPTIONS)
            .default(SecurityLevel::hybrid_policy_index_for_value(&seed.hybrid_policy) as usize)
            .pick()?;
        let hybrid_policy = SecurityLevel::hybrid_policy_from_index(selected_hybrid_idx);

        config.security = SecurityLevel::custom_with_thresholds(
            detector,
            recognizer,
            rgb_threshold,
            ir_threshold,
            hybrid_policy,
        );
    };

    Ok(())
}

fn edit_acceleration(
    term: &Term,
    theme: &ColorfulTheme,
    config: &mut Config,
) -> anyhow::Result<()> {
    let _ = term;
    if config.inference.is_representable() {
        let selected_execution_provider = Select::with_theme(theme)
            .with_prompt("Hardware acceleration (vendor runtime required; CPU is recommended)")
            .items(gaze_core::config::INFERENCE_EXECUTION_PROVIDER_LABELS)
            .default(config.inference.execution_provider_index() as usize)
            .pick()?;
        config.inference.execution_provider =
            gaze_core::config::InferenceConfig::execution_provider_from_index(
                selected_execution_provider,
            )
            .to_string();

        if config.inference.execution_provider == "openvino" {
            let selected_device = Select::with_theme(theme)
                .with_prompt("OpenVINO inference device")
                .items(INFERENCE_DEVICE_OPTIONS)
                .default(config.inference.device_index() as usize)
                .pick()?;
            config.inference.device =
                gaze_core::config::InferenceConfig::device_from_index(selected_device).to_string();
        } else {
            config.inference.device = if matches!(
                config.inference.execution_provider.as_str(),
                "auto" | "vitis"
            ) {
                "npu"
            } else {
                "cpu"
            }
            .to_string();
        }
    } else {
        term.write_line(&format!(
            "{} Keeping inference {}/{}: this build cannot change it",
            style("!").yellow().bold(),
            config.inference.execution_provider,
            config.inference.device
        ))?;
    }

    Ok(())
}

fn edit_authentication(
    term: &Term,
    theme: &ColorfulTheme,
    config: &mut Config,
) -> anyhow::Result<()> {
    let _ = term;
    term.write_line("On KDE lock/login screens, requiring confirmation uses your password instead of face unlock.")?;
    config.auth.require_confirmation_lock_screen = Confirm::with_theme(theme)
        .with_prompt(
            "Require confirmation (press Enter/Authenticate/OK) on the lock screen after face matches",
        )
        .default(config.auth.require_confirmation_lock_screen)
        .pick()?;

    config.auth.require_confirmation_elevation = Confirm::with_theme(theme)
        .with_prompt(
            "Require confirmation (press Enter/Authenticate/OK) for elevated auth (sudo, polkit, etc.) after face matches",
        )
        .default(config.auth.require_confirmation_elevation)
        .pick()?;

    config.auth.start_delay_ms = Input::with_theme(theme)
        .with_prompt("Start delay in milliseconds (0 disables)")
        .default(config.auth.start_delay_ms)
        .interact_text()?;

    if config.auth.start_delay_ms > 0 {
        let scope_index = Select::with_theme(theme)
            .with_prompt("Apply the start delay to")
            .items(START_DELAY_SCOPE_LABELS)
            .default(
                AuthConfig::start_delay_scope_index_for_value(config.auth.start_delay_scope())
                    as usize,
            )
            .pick()?;
        config.auth.start_delay_scope = AuthConfig::start_delay_scope_from_index(scope_index);
    }

    Ok(())
}

fn edit_authentication_tuning(
    term: &Term,
    theme: &ColorfulTheme,
    config: &mut Config,
) -> anyhow::Result<()> {
    let _ = term;
    config.auth.abort_if_ssh = Confirm::with_theme(theme)
        .with_prompt("Abort face authentication for SSH sessions")
        .default(config.auth.abort_if_ssh)
        .pick()?;

    config.auth.abort_if_lid_closed = Confirm::with_theme(theme)
        .with_prompt("Abort face authentication when laptop lid is closed")
        .default(config.auth.abort_if_lid_closed)
        .pick()?;

    config.auth.abort_before_first_resume = Confirm::with_theme(theme)
        .with_prompt("Abort face authentication until the system has suspended and resumed once")
        .default(config.auth.abort_before_first_resume)
        .pick()?;

    config.auth.resume_grace_ms = Input::with_theme(theme)
        .with_prompt("Resume grace period in milliseconds (delay auth after suspend)")
        .default(config.auth.resume_grace_ms)
        .interact_text()?;

    Ok(())
}

fn edit_enrollment(term: &Term, theme: &ColorfulTheme, config: &mut Config) -> anyhow::Result<()> {
    let _ = term;
    config.enrollment.max_templates = Input::with_theme(theme)
        .with_prompt("Max templates (sets of captures, 0 for no limit)")
        .default(config.enrollment.max_templates)
        .interact_text()?;

    let min_face_size_ratio: f64 = Input::with_theme(theme)
        .with_prompt("Minimum enrollment face size ratio (0.10 - 0.75; lower allows more distance)")
        .default(config.enrollment.min_face_size_ratio)
        .validate_with(|value: &f64| {
            if (MIN_ENROLLMENT_FACE_SIZE_RATIO..=MAX_ENROLLMENT_FACE_SIZE_RATIO).contains(value) {
                Ok(())
            } else {
                Err(format!(
                    "must be between {MIN_ENROLLMENT_FACE_SIZE_RATIO} and {MAX_ENROLLMENT_FACE_SIZE_RATIO}"
                ))
            }
        })
        .interact_text()?;
    config.enrollment.min_face_size_ratio = min_face_size_ratio;

    Ok(())
}

fn edit_liveness(term: &Term, theme: &ColorfulTheme, config: &mut Config) -> anyhow::Result<()> {
    let _ = term;
    config.liveness.enabled = Confirm::with_theme(theme)
        .with_prompt("Enable liveness anti-spoofing")
        .default(config.liveness.enabled)
        .pick()?;
    if config.liveness.enabled {
        config.liveness.threshold = Input::<String>::with_theme(theme)
            .with_prompt(format!(
                "Liveness threshold ({} - {})",
                MIN_LIVENESS_THRESHOLD, MAX_LIVENESS_THRESHOLD
            ))
            .default(config.liveness.threshold.to_string())
            .validate_with(|input: &String| match input.trim().parse::<f64>() {
                Ok(value)
                    if value.is_finite()
                        && (MIN_LIVENESS_THRESHOLD..=MAX_LIVENESS_THRESHOLD).contains(&value) =>
                {
                    Ok(())
                }
                Ok(_) => Err(format!(
                    "must be between {MIN_LIVENESS_THRESHOLD} and {MAX_LIVENESS_THRESHOLD}"
                )),
                Err(_) => Err("must be a number".to_string()),
            })
            .interact_text()?
            .trim()
            .parse::<f64>()
            .unwrap_or(0.8);
        config.liveness.max_seconds = Input::with_theme(theme)
            .with_prompt(format!(
                "Liveness max seconds ({MIN_LIVENESS_MAX_SECONDS}..={MAX_LIVENESS_MAX_SECONDS})"
            ))
            .default(config.liveness.max_seconds)
            .validate_with(|value: &f64| {
                if *value >= MIN_LIVENESS_MAX_SECONDS && *value <= MAX_LIVENESS_MAX_SECONDS {
                    Ok(())
                } else {
                    Err(format!(
                        "must be between {MIN_LIVENESS_MAX_SECONDS} and {MAX_LIVENESS_MAX_SECONDS}"
                    ))
                }
            })
            .interact_text()?;
    }

    Ok(())
}

fn edit_storage(
    term: &Term,
    theme: &ColorfulTheme,
    config: &mut Config,
    keyring_supported: gaze_core::dbus::KeyringSupport,
) -> anyhow::Result<()> {
    let _ = term;
    config.storage.encrypt_templates = Confirm::with_theme(theme)
        .with_prompt("Encrypt face templates at rest using TPM 2.0")
        .default(config.storage.encrypt_templates)
        .pick()?;

    let keyring_prerequisites = config.storage.encrypt_templates && config.liveness.enabled;
    for (was_on, name) in [
        (config.storage.unlock_gnome_keyring, "GNOME Keyring"),
        (config.storage.unlock_kwallet, "KWallet"),
    ] {
        if was_on && !keyring_prerequisites {
            term.write_line(&format!(
                "{} Turning off {name} unlock: it needs both template encryption and liveness",
                style("!").yellow().bold()
            ))?;
        }
    }

    config.storage.unlock_gnome_keyring =
        if keyring_supported.gnome && config.storage.encrypt_templates && config.liveness.enabled {
            Confirm::with_theme(theme)
                .with_prompt("Enable TPM-backed GNOME Keyring unlock for GDM or greetd face logins")
                .default(config.storage.unlock_gnome_keyring)
                .pick()?
        } else {
            false
        };

    config.storage.unlock_kwallet =
        if keyring_supported.kwallet && config.storage.encrypt_templates && config.liveness.enabled
        {
            Confirm::with_theme(theme)
                .with_prompt("Enable TPM-backed KWallet unlock for KDE face logins")
                .default(config.storage.unlock_kwallet)
                .pick()?
        } else {
            false
        };

    Ok(())
}

fn edit_camera_tuning(
    term: &Term,
    theme: &ColorfulTheme,
    config: &mut Config,
) -> anyhow::Result<()> {
    let _ = term;
    config.cameras.dark_luma_threshold = Input::<u8>::with_theme(theme)
        .with_prompt("Darkness cutoff: reject frames below this mean brightness (0-255)")
        .default(config.cameras.dark_luma_threshold)
        .interact_text()?;

    if config.cameras.ir.is_empty() {
        config.cameras.emitter_enabled = false;
        config.cameras.parallel_capture = "never".to_string();
    } else {
        config.cameras.emitter_enabled = Confirm::with_theme(theme)
            .with_prompt("Force IR emitter override (only use if emitter stays off automatically)")
            .default(config.cameras.emitter_enabled)
            .pick()?;

        if config.cameras.rgb.is_empty() {
            config.cameras.parallel_capture = "never".to_string();
        } else {
            let capture_idx = Select::with_theme(theme)
                .with_prompt(
                    "Capture RGB and IR at the same time (faster, but some webcams cannot)",
                )
                .items(gaze_core::config::PARALLEL_CAPTURE_LABELS.as_slice())
                .default(config.cameras.parallel_capture_index() as usize)
                .pick()?;
            config.cameras.parallel_capture =
                gaze_core::config::CameraConfig::parallel_capture_from_index(capture_idx);
        }
    }

    Ok(())
}

// The wizard runs unprivileged, but storing a credential needs root, which `gaze keyring` obtains.
fn enroll_keyring(config: &Config, backend: gaze_security::keyring::Backend) -> anyhow::Result<()> {
    if crate::is_root() {
        return keyring::enroll(&get_current_user(), config, backend);
    }
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg("keyring");
    if backend == gaze_security::keyring::Backend::KWallet {
        command.arg("--kwallet");
    }
    let status = command.status()?;
    anyhow::ensure!(
        status.success(),
        "{} password enrollment did not complete",
        backend.name()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_only_lists_changed_fields_and_keeps_sections() {
        let before = Config::default();
        let mut after = before.clone();
        after.auth.start_delay_ms = 3000;
        after.cameras.dark_luma_threshold = 30;
        let changes = change_summary(&before, &after).unwrap();
        assert_eq!(changes.len(), 2);
        assert!(
            changes
                .iter()
                .any(|line| line == "auth.start_delay_ms: 0 → 3000")
        );
        assert!(
            changes
                .iter()
                .any(|line| line == "cameras.dark_luma_threshold: 20 → 30")
        );
        assert!(change_summary(&before, &before).unwrap().is_empty());
    }

    #[test]
    fn a_configured_camera_that_is_not_enumerated_is_still_offered() {
        let mut options = vec![("Built-in".to_string(), "/dev/video0".to_string())];

        ensure_configured_source_listed(&mut options, "/dev/video9");

        assert_eq!(options.len(), 2);
        assert_eq!(
            options[1],
            (
                "/dev/video9 (configured)".to_string(),
                "/dev/video9".to_string()
            ),
            "a camera that is unplugged right now must not silently change"
        );
    }

    #[test]
    fn an_enumerated_camera_is_not_listed_twice() {
        let mut options = vec![("Built-in".to_string(), "/dev/video0".to_string())];

        ensure_configured_source_listed(&mut options, "/dev/video0");
        ensure_configured_source_listed(&mut options, "  /dev/video0  ");

        assert_eq!(options.len(), 1);
    }

    #[test]
    fn an_unset_camera_adds_no_option() {
        let mut options = vec![("Built-in".to_string(), "/dev/video0".to_string())];

        ensure_configured_source_listed(&mut options, "");
        ensure_configured_source_listed(&mut options, "   ");

        assert_eq!(options.len(), 1);
    }
}
