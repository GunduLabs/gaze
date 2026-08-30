// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

mod doctor;
mod polkit;
mod tui;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::CompleteEnv;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use console::{Term, style};
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use futures::StreamExt;
use gaze_core::config::{
    AuthConfig, Config, DEFAULT_SECURITY_THRESHOLD, HYBRID_POLICY_OPTIONS,
    INFERENCE_DEVICE_OPTIONS, INFERENCE_EXECUTION_PROVIDER_OPTIONS, MAX_ENROLLMENT_FACE_SIZE_RATIO,
    MAX_LIVENESS_THRESHOLD, MAX_SECURITY_THRESHOLD, MIN_ENROLLMENT_FACE_SIZE_RATIO,
    MIN_LIVENESS_MAX_FRAMES, MIN_LIVENESS_THRESHOLD, MIN_SECURITY_THRESHOLD, MODEL_QUALITY_OPTIONS,
    SECURITY_LEVEL_OPTIONS, START_DELAY_SCOPE_LABELS, SecurityLevel,
};
use gaze_core::dbus::{
    CaptureStatus, EnrollPrompt, GazeProxy, VerifyResult, apply_config_to_daemon, connect_gaze,
    dbus_error_message, dbus_is_file_not_found, load_config_from_daemon,
};
use std::{future::Future, time::Duration};
use tui::{AuthScreen, BusyScreen, EnrollScreen, Tone, TuiAction, TuiTerminal};

fn is_root() -> bool {
    (unsafe { libc::geteuid() }) == 0
}

fn resolve_current_user(sudo_user: Option<String>, user: Option<String>) -> String {
    [sudo_user, user]
        .into_iter()
        .flatten()
        .find(|value| !value.is_empty())
        .unwrap_or_else(|| "root".into())
}

fn get_current_user() -> String {
    let sudo_user = is_root().then(|| std::env::var("SUDO_USER").ok()).flatten();
    resolve_current_user(sudo_user, std::env::var("USER").ok())
}

fn face_completer(current: &std::ffi::OsStr) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };
    let Ok(rt) = tokio::runtime::Runtime::new() else {
        return Vec::new();
    };
    rt.block_on(async {
        let Ok(proxy) = connect_gaze().await else {
            return Vec::new();
        };
        let Ok(faces) = proxy.list_faces(&get_current_user()).await else {
            return Vec::new();
        };
        faces
            .into_iter()
            .filter(|(face, ..)| face.starts_with(current))
            .map(|(face, ..)| CompletionCandidate::new(face))
            .collect()
    })
}

fn command_requires_root(command: &Commands) -> Option<&'static str> {
    match command {
        Commands::AddFace { .. } => Some("add-face"),
        Commands::RefineFace { .. } => Some("refine-face"),
        Commands::RemoveFace { .. } => Some("remove-face"),
        Commands::RenameFace { .. } => Some("rename-face"),
        Commands::ClearUser { .. } => Some("clear-user"),
        Commands::Config { show } => (!show).then_some("config"),
        Commands::Auth { .. }
        | Commands::ListFaces { .. }
        | Commands::Doctor { .. }
        | Commands::Uninstall { .. } => None,
    }
}

fn command_target_user(command: &Commands) -> Option<&str> {
    match command {
        Commands::Auth { user, .. }
        | Commands::ListFaces { user }
        | Commands::Doctor { user, .. } => user.as_deref(),
        _ => None,
    }
}

fn command_may_be_challenged(command: &Commands) -> bool {
    !is_root() && matches!(command_target_user(command), Some(user) if user != get_current_user())
}

const ESCALATION_MARKER: &str = "GAZE_ESCALATED";
const ESCALATION_PRESERVED_ENV: [&str; 1] = ["XDG_RUNTIME_DIR"];

fn reexec_as_root(name: &str) -> anyhow::Result<()> {
    if std::env::var_os(ESCALATION_MARKER).is_some() {
        anyhow::bail!("gaze {name} re-ran itself but did not gain root privileges");
    }
    if !which("sudo") {
        anyhow::bail!("gaze {name} needs root privileges, but sudo was not found");
    }

    let mut cmd = std::process::Command::new("sudo");
    cmd.arg("--")
        .arg("env")
        .arg(format!("{ESCALATION_MARKER}=1"));
    for key in ESCALATION_PRESERVED_ENV {
        if let Some(value) = std::env::var_os(key) {
            let mut pair = std::ffi::OsString::from(key);
            pair.push("=");
            pair.push(value);
            cmd.arg(pair);
        }
    }
    cmd.arg(std::env::current_exe()?)
        .args(std::env::args_os().skip(1));

    let status = cmd.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn capture_tone(status: CaptureStatus) -> Tone {
    match status {
        CaptureStatus::Ready | CaptureStatus::Usable => Tone::Good,
        CaptureStatus::Unused | CaptureStatus::NoFace => Tone::Error,
        CaptureStatus::TooDark
        | CaptureStatus::Clipped
        | CaptureStatus::NotCentered
        | CaptureStatus::TooFar
        | CaptureStatus::TooClose => Tone::Warn,
    }
}

fn interactive_terminal() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && std::io::stdin().is_terminal()
}

async fn run_busy<F, T>(title: &str, message: String, tone: Tone, future: F) -> anyhow::Result<T>
where
    F: Future<Output = T>,
{
    if !interactive_terminal() {
        return Ok(future.await);
    }

    let mut terminal = TuiTerminal::new()?;
    let mut tick = 0_u64;
    tokio::pin!(future);

    loop {
        terminal.draw_busy(&BusyScreen {
            title,
            message: &message,
            tone,
            tick,
        })?;
        if let Some(TuiAction::Cancel) = tui::poll_action()? {
            drop(terminal);
            anyhow::bail!("cancelled");
        }

        tokio::select! {
            result = &mut future => {
                drop(terminal);
                return Ok(result);
            }
            _ = tokio::time::sleep(Duration::from_millis(80)) => {
                tick = tick.wrapping_add(1);
            }
        }
    }
}

#[derive(Parser)]
#[command(name = "gaze", version, about = "CLI for Gaze")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Authenticate a user via webcam
    Auth {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(
            short,
            long,
            conflicts_with = "silent",
            help = "Show detailed authentication metrics"
        )]
        verbose: bool,
        #[arg(
            short,
            long,
            help = "Suppress the terminal UI and all output; report the result via exit code"
        )]
        silent: bool,
    },
    /// Capture a new face with guided multi-angle template
    AddFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "The name of the face to enroll")]
        face: String,
    },
    /// Add additional captures to improve recognition of an existing face
    RefineFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "The name of the face to refine", add = ArgValueCompleter::new(face_completer))]
        face: String,
    },
    /// List all faces enrolled for a user
    ListFaces {
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Remove a named face for a user
    RemoveFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "The name of the face to remove", add = ArgValueCompleter::new(face_completer))]
        face: String,
    },
    /// Rename a face for a user
    RenameFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "Current face name", add = ArgValueCompleter::new(face_completer))]
        from: String,
        #[arg(help = "New face name")]
        to: String,
    },
    /// Remove all data for a user
    ClearUser {
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Interactive configuration editor for daemon and GDM options
    Config {
        #[arg(long, help = "Print current values and exit")]
        show: bool,
    },
    /// Check the Gaze installation for configuration and runtime problems
    Doctor {
        #[arg(short, long, help = "Check enrollments for this user")]
        user: Option<String>,
        #[arg(
            short,
            long,
            help = "Benchmark detector, recognizer, and liveness model inference speed"
        )]
        benchmark: bool,
    },
    /// Completely uninstall Gaze: packages, PAM integration, config, models, and user data
    Uninstall {
        #[arg(short = 'y', long, help = "Skip the confirmation prompt")]
        yes: bool,
        #[arg(long, help = "Preserve /var/lib/gaze (enrolled face data)")]
        keep_data: bool,
        #[arg(long, help = "Print the planned commands without executing them")]
        dry_run: bool,
    },
}

fn ensure_configured_source_listed(options: &mut Vec<(String, String)>, configured: &str) {
    let configured = configured.trim();
    if configured.is_empty() || gaze_core::camera::is_listed_source(options, configured) {
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

async fn run_config_wizard(
    term: &Term,
    proxy: &GazeProxy<'_>,
    mut config: Config,
) -> anyhow::Result<()> {
    let theme = ColorfulTheme::default();

    term.write_line(&format!(
        "\n{}\n",
        style("Gaze Config Wizard").cyan().bold()
    ))?;

    let selected = Select::with_theme(&theme)
        .with_prompt("Security level")
        .items(SECURITY_LEVEL_OPTIONS)
        .default(config.security.level_index() as usize)
        .interact()?;

    if let Some(level) = SecurityLevel::preset_from_index(selected) {
        config.security = level;
    } else {
        let seed = config.security.custom_form();

        let selected_det_idx = Select::with_theme(&theme)
            .with_prompt("Custom detector level")
            .items(MODEL_QUALITY_OPTIONS)
            .default(SecurityLevel::model_quality_index(&seed.detector) as usize)
            .interact()?;
        let detector = SecurityLevel::model_quality_from_index(selected_det_idx).to_string();

        let selected_rec_idx = Select::with_theme(&theme)
            .with_prompt("Custom recognizer level")
            .items(MODEL_QUALITY_OPTIONS)
            .default(SecurityLevel::model_quality_index(&seed.recognizer) as usize)
            .interact()?;
        let recognizer = SecurityLevel::model_quality_from_index(selected_rec_idx).to_string();

        let rgb_threshold = prompt_security_threshold(&theme, "RGB", seed.rgb_threshold)?;
        let ir_threshold = prompt_security_threshold(&theme, "IR", seed.ir_threshold)?;

        let selected_hybrid_idx = Select::with_theme(&theme)
            .with_prompt("Custom hybrid combining policy")
            .items(HYBRID_POLICY_OPTIONS)
            .default(SecurityLevel::hybrid_policy_index_for_value(&seed.hybrid_policy) as usize)
            .interact()?;
        let hybrid_policy = SecurityLevel::hybrid_policy_from_index(selected_hybrid_idx);

        config.security = SecurityLevel::custom_with_thresholds(
            detector,
            recognizer,
            rgb_threshold,
            ir_threshold,
            hybrid_policy,
        );
    };

    if config.inference.is_representable() {
        let selected_execution_provider = Select::with_theme(&theme)
            .with_prompt("Inference execution provider")
            .items(INFERENCE_EXECUTION_PROVIDER_OPTIONS)
            .default(config.inference.execution_provider_index() as usize)
            .interact()?;
        config.inference.execution_provider =
            gaze_core::config::InferenceConfig::execution_provider_from_index(
                selected_execution_provider,
            )
            .to_string();

        if config.inference.execution_provider == "openvino" {
            let selected_device = Select::with_theme(&theme)
                .with_prompt("OpenVINO inference device")
                .items(INFERENCE_DEVICE_OPTIONS)
                .default(config.inference.device_index() as usize)
                .interact()?;
            config.inference.device =
                gaze_core::config::InferenceConfig::device_from_index(selected_device).to_string();
        } else {
            config.inference.device = "cpu".to_string();
        }
    } else {
        term.write_line(&format!(
            "{} Keeping inference {}/{}: this build cannot change it",
            style("!").yellow().bold(),
            config.inference.execution_provider,
            config.inference.device
        ))?;
    }

    let mut cameras = gaze_core::camera::enumerate_cameras().unwrap_or_default();
    if cameras.is_empty() {
        anyhow::bail!("No PipeWire cameras detected! Please ensure your video inputs are active.");
    }
    ensure_configured_source_listed(&mut cameras, &config.cameras.rgb);
    let cam_names: Vec<String> = cameras.iter().map(|(n, _)| n.clone()).collect();
    let default_cam_idx = gaze_core::camera::source_index(&cameras, &config.cameras.rgb);

    let selected_cam_idx = Select::with_theme(&theme)
        .with_prompt("RGB camera source")
        .items(&cam_names)
        .default(default_cam_idx)
        .interact()?;

    config.cameras.rgb = cameras[selected_cam_idx].1.clone();

    config.cameras.dark_luma_threshold = Input::<u8>::with_theme(&theme)
        .with_prompt("Darkness cutoff: reject frames below this mean brightness (0-255)")
        .default(config.cameras.dark_luma_threshold)
        .interact_text()?;

    let mut ir_options = gaze_core::camera::ir_choices();
    ensure_configured_source_listed(&mut ir_options, &config.cameras.ir);

    let ir_names: Vec<String> = ir_options.iter().map(|(n, _)| n.clone()).collect();
    let default_ir_idx = gaze_core::camera::source_index(&ir_options, &config.cameras.ir);

    let selected_ir_idx = Select::with_theme(&theme)
        .with_prompt("IR camera source")
        .items(&ir_names)
        .default(default_ir_idx)
        .interact()?;

    config.cameras.ir = ir_options[selected_ir_idx].1.clone();

    if config.cameras.ir.is_empty() {
        config.cameras.emitter_enabled = false;
        config.cameras.parallel_capture = "never".to_string();
    } else {
        config.cameras.emitter_enabled = Confirm::with_theme(&theme)
            .with_prompt("Force IR emitter override (only use if emitter stays off automatically)")
            .default(config.cameras.emitter_enabled)
            .interact()?;

        let capture_idx = Select::with_theme(&theme)
            .with_prompt("Capture RGB and IR at the same time (faster, but some webcams cannot)")
            .items(gaze_core::config::PARALLEL_CAPTURE_LABELS.as_slice())
            .default(config.cameras.parallel_capture_index() as usize)
            .interact()?;
        config.cameras.parallel_capture =
            gaze_core::config::CameraConfig::parallel_capture_from_index(capture_idx);
    }

    config.auth.abort_if_ssh = Confirm::with_theme(&theme)
        .with_prompt("Abort face auth for SSH sessions")
        .default(config.auth.abort_if_ssh)
        .interact()?;

    config.auth.abort_if_lid_closed = Confirm::with_theme(&theme)
        .with_prompt("Abort face auth when laptop lid is closed")
        .default(config.auth.abort_if_lid_closed)
        .interact()?;

    config.auth.abort_before_first_resume = Confirm::with_theme(&theme)
        .with_prompt("Abort face auth until the system has suspended and resumed once")
        .default(config.auth.abort_before_first_resume)
        .interact()?;

    config.auth.require_confirmation_lock_screen = Confirm::with_theme(&theme)
        .with_prompt(
            "Require confirmation (press Enter/Authenticate/OK) on the lock screen after face matches",
        )
        .default(config.auth.require_confirmation_lock_screen)
        .interact()?;

    config.auth.require_confirmation_elevation = Confirm::with_theme(&theme)
        .with_prompt(
            "Require confirmation (press Enter/Authenticate/OK) for elevated auth (sudo, polkit, etc.) after face matches",
        )
        .default(config.auth.require_confirmation_elevation)
        .interact()?;

    config.auth.resume_grace_ms = Input::with_theme(&theme)
        .with_prompt("Resume grace period in milliseconds (delay auth after suspend)")
        .default(config.auth.resume_grace_ms)
        .interact_text()?;

    config.auth.start_delay_ms = Input::with_theme(&theme)
        .with_prompt("Start delay in milliseconds (0 disables)")
        .default(config.auth.start_delay_ms)
        .interact_text()?;

    if config.auth.start_delay_ms > 0 {
        let scope_index = Select::with_theme(&theme)
            .with_prompt("Apply the start delay to")
            .items(START_DELAY_SCOPE_LABELS)
            .default(
                AuthConfig::start_delay_scope_index_for_value(config.auth.start_delay_scope())
                    as usize,
            )
            .interact()?;
        config.auth.start_delay_scope = AuthConfig::start_delay_scope_from_index(scope_index);
    }

    config.enrollment.max_templates = Input::with_theme(&theme)
        .with_prompt("Max templates (sets of captures)")
        .default(config.enrollment.max_templates)
        .interact_text()?;

    let min_face_size_ratio: f64 = Input::with_theme(&theme)
        .with_prompt("Minimum enrollment face size ratio (0.10 - 0.75; lower allows more distance)")
        .default(config.enrollment.min_face_size_ratio)
        .interact_text()?;
    config.enrollment.min_face_size_ratio = if min_face_size_ratio.is_finite() {
        min_face_size_ratio.clamp(
            MIN_ENROLLMENT_FACE_SIZE_RATIO,
            MAX_ENROLLMENT_FACE_SIZE_RATIO,
        )
    } else {
        config.enrollment.effective_min_face_size_ratio() as f64
    };

    config.liveness.enabled = Confirm::with_theme(&theme)
        .with_prompt("Enable liveness anti-spoofing")
        .default(config.liveness.enabled)
        .interact()?;
    if config.liveness.enabled {
        config.liveness.threshold = Input::<String>::with_theme(&theme)
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
        config.liveness.max_frames = Input::with_theme(&theme)
            .with_prompt(format!(
                "Liveness max frames (min {MIN_LIVENESS_MAX_FRAMES})"
            ))
            .default(config.liveness.max_frames)
            .validate_with(|value: &u32| {
                if *value >= MIN_LIVENESS_MAX_FRAMES {
                    Ok(())
                } else {
                    Err(format!("must be at least {MIN_LIVENESS_MAX_FRAMES}"))
                }
            })
            .interact_text()?;
    }

    config.storage.encrypt_templates = Confirm::with_theme(&theme)
        .with_prompt("Encrypt face templates at rest using TPM 2.0")
        .default(config.storage.encrypt_templates)
        .interact()?;

    apply_config_to_daemon(proxy, &config).await?;
    term.write_line(&format!(
        "{} Configuration saved. Daemon will restart to apply changes.",
        style("✓").green().bold()
    ))?;

    Ok(())
}

async fn handle_enroll(
    proxy: &GazeProxy<'_>,
    user: &str,
    face: &str,
    is_refine: bool,
) -> anyhow::Result<()> {
    let term = Term::stdout();

    if let Err(err) = proxy.claim(user).await {
        term.write_line(&format!(
            "{} Failed to claim device: {}",
            style("✗").red().bold(),
            dbus_error_message(&err)
        ))?;
        std::process::exit(1);
    }

    let mut enroll_stream = proxy.receive_enroll_status().await?;
    let mut capture_stream = proxy.receive_face_status().await?;
    let mut terminal = match TuiTerminal::new() {
        Ok(terminal) => terminal,
        Err(err) => {
            let _ = proxy.release().await;
            return Err(err);
        }
    };
    if let Err(err) = proxy.enroll_start(face).await {
        drop(terminal);
        let _ = proxy.release().await;
        anyhow::bail!("Failed to start enrollment: {}", dbus_error_message(&err));
    }

    let mut current_enroll_msg = "Waiting for capture prompt".to_string();
    let mut current_capture_msg = "Waiting for face...".to_string();
    let mut current_capture_tone = Tone::Info;
    let mut current_progress = 0_u32;
    let mut current_max = 100_u32;
    let mut current_time_remaining = None;
    let mut confirm_cancel = false;
    let mut tick = 0_u64;

    let mut is_cancelled = false;
    let mut is_completed = false;
    let mut is_failed = false;
    loop {
        terminal.draw_enroll(&EnrollScreen {
            user,
            face,
            is_refine,
            prompt: &current_enroll_msg,
            capture: &current_capture_msg,
            capture_tone: current_capture_tone,
            progress: current_progress,
            max: current_max,
            time_remaining: current_time_remaining,
            confirm_cancel,
            tick,
        })?;

        if tui::apply_cancel_action(&mut confirm_cancel, tui::poll_action()?)
            == tui::ConfirmStep::CancelConfirmed
        {
            is_cancelled = true;
            break;
        }

        tokio::select! {
            signal = enroll_stream.next() => match signal {
                Some(signal) => {
                    if let Ok(args) = signal.args() {
                        let raw_msg = *args.msg();
                        let time_remaining = *args.time_remaining();
                        let is_done = *args.is_done();
                        current_progress = *args.progress();
                        current_max = *args.max();
                        current_enroll_msg = raw_msg.to_string();
                        current_time_remaining = (time_remaining > 0.0).then_some(time_remaining);

                        if matches!(raw_msg, EnrollPrompt::DbFailed | EnrollPrompt::CameraFailed | EnrollPrompt::Cancelled) {
                            is_failed = true;
                            break;
                        }

                        if is_done && raw_msg == EnrollPrompt::Completed {
                            is_completed = true;
                            break;
                        }

                        if is_done {
                            is_failed = true;
                            break;
                        }
                    }
                }
                None => {
                    is_failed = true;
                    break;
                }
            },
            signal = capture_stream.next() => {
                if let Some(signal) = signal
                    && let Ok(args) = signal.args()
                {
                    let status = *args.status();
                    current_capture_msg = status.to_string();
                    current_capture_tone = capture_tone(status);
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(80)) => {
                tick = tick.wrapping_add(1);
            }
        }
    }

    drop(terminal);

    if is_cancelled {
        let _ = proxy.enroll_stop().await;
    }
    let _ = proxy.release().await;
    if is_cancelled {
        term.write_line(&format!(
            "\n{} Enrollment cancelled",
            style("✗").red().bold()
        ))?;
        std::process::exit(130);
    }
    if is_completed {
        term.write_line(&format!(
            "  {} Captures saved for {}/{}!\n",
            style("✓").green().bold(),
            style(user).green(),
            style(face).green()
        ))?;
    } else if is_failed {
        term.write_line(&format!(
            "{} Enrollment failed: {}",
            style("✗").red().bold(),
            current_enroll_msg
        ))?;
        std::process::exit(1);
    }
    Ok(())
}

async fn handle_auth(
    proxy: &GazeProxy<'_>,
    user: &str,
    verbose: bool,
    silent: bool,
) -> anyhow::Result<()> {
    let term = Term::stdout();

    let has_faces = match proxy.list_faces(user).await {
        Ok(faces) => !faces.is_empty(),
        Err(ref e) if gaze_core::dbus::dbus_is_file_not_found(e) => false,
        Err(e) => return Err(e.into()),
    };
    if !has_faces {
        if !silent {
            term.write_line(&format!(
                "{} No faces enrolled for {}. Run {} to enroll a face.",
                style("i").cyan().bold(),
                style(user).bold(),
                style("gaze add-face <name>").bold()
            ))?;
        }
        std::process::exit(1);
    }

    let start = std::time::Instant::now();

    if let Err(err) = proxy.claim(user).await {
        if !silent {
            term.write_line(&format!(
                "{} Failed to claim device: {}",
                style("✗").red().bold(),
                dbus_error_message(&err)
            ))?;
        }
        std::process::exit(1);
    }

    let mut status_stream = proxy.receive_verify_status().await?;
    let mut capture_stream = proxy.receive_face_status().await?;
    let mut diagnostic_stream = proxy.receive_verify_diagnostic().await?;
    let mut terminal = if !silent {
        match TuiTerminal::new() {
            Ok(terminal) => Some(terminal),
            Err(err) => {
                let _ = proxy.release().await;
                return Err(err);
            }
        }
    } else {
        None
    };

    if let Err(e) = proxy.verify_start("any").await {
        drop(terminal);
        if !silent {
            term.write_line(&format!("{} Daemon error: {}", style("✗").red().bold(), e))?;
        }
        let _ = proxy.release().await;
        std::process::exit(1);
    }

    let mut status_msg = format!("Scanning face for {user}...");
    let mut status_tone = Tone::Info;
    let mut tick = 0_u64;
    let mut cancelled = false;
    let mut timed_out = false;
    let mut verify_result = None;
    let mut diagnostics = Vec::new();
    let deadline = tokio::time::Instant::now() + gaze_core::dbus::VERIFY_CLIENT_TIMEOUT;

    loop {
        if let Some(ref mut terminal) = terminal {
            terminal.draw_auth(&AuthScreen {
                user,
                status: &status_msg,
                status_tone,
                elapsed: start.elapsed(),
                tick,
            })?;

            if let Some(TuiAction::Cancel) = tui::poll_action()? {
                cancelled = true;
                break;
            }
        }

        tokio::select! {
            signal = status_stream.next() => {
                let Some(signal) = signal else { break };
                if let Ok(args) = signal.args() {
                    verify_result = Some((*args.result(), args.faces().clone(), *args.rgb_status(), *args.ir_status()));
                    break;
                }
            }
            signal = capture_stream.next() => {
                let Some(signal) = signal else { break };
                if let Ok(args) = signal.args() {
                    let status = *args.status();
                    status_tone = capture_tone(status);
                    status_msg = match status {
                        CaptureStatus::Ready | CaptureStatus::Usable => format!("Scanning face for {user}..."),
                        _ => status.to_string(),
                    };
                }
            }
            // Collected even without `--verbose`: a failure explains itself with the last one.
            signal = diagnostic_stream.next() => {
                let Some(signal) = signal else { break };
                if let Ok(args) = signal.args() {
                    diagnostics.push(args.message().to_string());
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                break;
            }
            _ = tokio::time::sleep(Duration::from_millis(80)) => {
                tick = tick.wrapping_add(1);
            }
        }
    }

    drop(terminal);

    // A diagnostic sent just before the verdict can still be in flight, and it is the one that
    // says why: the streams are separate, so arrival order is not guaranteed.
    while let Ok(Some(signal)) =
        tokio::time::timeout(Duration::from_millis(20), diagnostic_stream.next()).await
    {
        if let Ok(args) = signal.args() {
            diagnostics.push(args.message().to_string());
        }
    }

    if cancelled {
        let _ = proxy.verify_stop().await;
        let _ = proxy.release().await;
        std::process::exit(130);
    }

    if timed_out {
        let _ = proxy.verify_stop().await;
        let _ = proxy.release().await;
        if !silent {
            term.write_line(&format!(
                "{} Timed out waiting for the daemon to decide ({}ms)",
                style("✗").red().bold(),
                start.elapsed().as_millis()
            ))?;
        }
        std::process::exit(1);
    }

    let mut authenticated = false;
    if let Some((result, faces, rgb_status, ir_status)) = verify_result {
        if verbose {
            for message in &diagnostics {
                println!("{message}");
            }
            if !diagnostics.is_empty() {
                println!();
            }
            println!(
                "\n{:<20} {:>10} {:>8} {:>8} {:>10} {:>8} {:>8}",
                style("Face").bold(),
                style("RGB Sim").bold(),
                style("RGB %").bold(),
                style("RGB Pass").bold(),
                style("IR Sim").bold(),
                style("IR %").bold(),
                style("IR Pass").bold()
            );
            println!("{}", style("-".repeat(78)).dim());
            for (name, rgb_sim, rgb_pct, rgb_passed, ir_sim, ir_pct, ir_passed) in &faces {
                let rgb_check = if *rgb_passed {
                    style("✓").green()
                } else {
                    style("✗").red()
                };
                let ir_check = if *ir_passed {
                    style("✓").green()
                } else {
                    style("✗").red()
                };
                println!(
                    "{:<20} {:>10.4} {:>7.1}% {:>8} {:>10.4} {:>7.1}% {:>8}",
                    style(name).cyan(),
                    rgb_sim,
                    rgb_pct,
                    rgb_check,
                    ir_sim,
                    ir_pct,
                    ir_check
                );
            }
            println!();

            println!(
                "{} RGB: {} | IR: {}",
                style("Status:").bold(),
                style(format!("{:?}", rgb_status)).cyan(),
                style(format!("{:?}", ir_status)).cyan()
            );
            println!();
        }

        if result == VerifyResult::VerifyMatch {
            authenticated = true;
            if !silent {
                let matched = faces
                    .iter()
                    .find(|(_, _, _, rgb_p, _, _, ir_p)| *rgb_p || *ir_p)
                    .map(|(n, _, rgb_pct, rgb_p, _, ir_pct, ir_p)| {
                        let pct = if *rgb_p && *ir_p {
                            rgb_pct.max(*ir_pct)
                        } else if *rgb_p {
                            *rgb_pct
                        } else {
                            *ir_pct
                        };
                        (n.clone(), pct)
                    });
                if let Some((face, pct)) = matched {
                    term.write_line(&format!(
                        "{} Authenticated as: {} ({:.1}%, {}ms)",
                        style("✓").green().bold(),
                        style(&face).green().bold(),
                        pct,
                        start.elapsed().as_millis()
                    ))?;
                } else {
                    term.write_line(&format!(
                        "{} Authenticated as: {} ({}ms)",
                        style("✓").green().bold(),
                        style(user).green().bold(),
                        start.elapsed().as_millis()
                    ))?;
                }
            }
        }
    }

    if !authenticated && !silent {
        term.write_line(&format!(
            "{} Authentication failed ({}ms)",
            style("✗").red().bold(),
            start.elapsed().as_millis()
        ))?;
        // "Authentication failed" on its own reads as a face that was not recognised, even when
        // the camera never opened. Verbose mode has already printed the whole list.
        if !verbose && let Some(reason) = diagnostics.last() {
            term.write_line(&format!("  {}", style(reason).yellow()))?;
        }
    }

    let _ = proxy.release().await;
    if !authenticated {
        std::process::exit(1);
    }
    Ok(())
}

async fn handle_list_faces(proxy: &GazeProxy<'_>, user: &str) -> anyhow::Result<()> {
    let term = Term::stdout();
    let result = run_busy(
        "Face database",
        format!("Fetching faces for {user}..."),
        Tone::Info,
        proxy.list_faces(user),
    )
    .await?;

    match result {
        Ok(faces) => {
            if faces.is_empty() {
                term.write_line(&format!(
                    "{} No faces found for {}",
                    style("i").cyan().bold(),
                    style(user).bold()
                ))?;
            } else {
                term.write_line(&format!(
                    "\n{} face{} for {}:\n",
                    style(faces.len()).green().bold(),
                    if faces.len() == 1 { "" } else { "s" },
                    style(user).bold()
                ))?;
                for (face, count, has_rgb, has_ir) in faces {
                    let rgb_badge = if has_rgb {
                        style("[RGB]").green().bold().to_string()
                    } else {
                        style("[RGB]").red().bold().to_string()
                    };
                    let ir_badge = if has_ir {
                        style("[IR]").green().bold().to_string()
                    } else {
                        style("[IR]").red().bold().to_string()
                    };
                    term.write_line(&format!(
                        "  {} {} {} {} ({} capture{})",
                        style("•").cyan(),
                        style(face).bold(),
                        rgb_badge,
                        ir_badge,
                        count,
                        if count == 1 { "" } else { "s" }
                    ))?;
                }
                term.write_line("")?;
            }
        }
        Err(e) => {
            if dbus_is_file_not_found(&e) {
                term.write_line(&format!(
                    "{} No faces found for {}",
                    style("i").cyan().bold(),
                    style(user).bold()
                ))?;
            } else {
                term.write_line(&format!(
                    "{} Failed to fetch faces: {}",
                    style("✗").red().bold(),
                    dbus_error_message(&e)
                ))?;
                std::process::exit(1);
            }
        }
    }
    Ok(())
}

async fn handle_remove_face(proxy: &GazeProxy<'_>, user: &str, face: &str) -> anyhow::Result<()> {
    let term = Term::stdout();
    let result = run_busy(
        "Remove face",
        format!("Deleting face {face}..."),
        Tone::Warn,
        proxy.delete_face(user, face),
    )
    .await?;

    match result {
        Ok(true) => {
            term.write_line(&format!(
                "{} Face '{}' removed for '{}'",
                style("✓").green().bold(),
                face,
                user
            ))?;
        }
        Ok(false) => {
            term.write_line(&format!(
                "{} Face '{}' not found for '{}'",
                style("!").yellow().bold(),
                face,
                user
            ))?;
        }
        Err(err) => {
            term.write_line(&format!(
                "{} Failed to remove face: {}",
                style("✗").red().bold(),
                dbus_error_message(&err)
            ))?;
            std::process::exit(1);
        }
    }
    Ok(())
}

async fn handle_rename_face(
    proxy: &GazeProxy<'_>,
    user: &str,
    from: &str,
    to: &str,
) -> anyhow::Result<()> {
    let term = Term::stdout();
    let result = run_busy(
        "Rename face",
        format!("Renaming face {from} -> {to}..."),
        Tone::Info,
        proxy.rename_face(user, from, to),
    )
    .await?;

    match result {
        Ok(true) => {
            term.write_line(&format!(
                "{} Face '{}' renamed to '{}' for '{}'",
                style("✓").green().bold(),
                from,
                to,
                user
            ))?;
        }
        Ok(false) => {
            term.write_line(&format!(
                "{} Face '{}' not found for '{}'",
                style("!").yellow().bold(),
                from,
                user
            ))?;
        }
        Err(err) => {
            term.write_line(&format!(
                "{} Failed to rename face: {}",
                style("✗").red().bold(),
                dbus_error_message(&err)
            ))?;
            std::process::exit(1);
        }
    }
    Ok(())
}

async fn handle_clear_user(proxy: &GazeProxy<'_>, user: &str) -> anyhow::Result<()> {
    let term = Term::stdout();
    let result = run_busy(
        "Clear user",
        format!("Deleting all data for {user}..."),
        Tone::Warn,
        proxy.delete_faces(user),
    )
    .await?;

    match result {
        Ok(true) => {
            term.write_line(&format!(
                "{} All data cleared for '{}'",
                style("✓").green().bold(),
                user
            ))?;
        }
        Ok(false) => {
            term.write_line(&format!(
                "{} No data found for '{}'",
                style("!").yellow().bold(),
                user
            ))?;
        }
        Err(err) => {
            term.write_line(&format!(
                "{} Failed to clear user: {}",
                style("✗").red().bold(),
                dbus_error_message(&err)
            ))?;
            std::process::exit(1);
        }
    }
    Ok(())
}

fn which(bin: &str) -> bool {
    std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} >/dev/null 2>&1", bin))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn reset_gnome_user_settings_cmd() -> String {
    [
        "if command -v getent >/dev/null 2>&1 && command -v dbus-run-session >/dev/null 2>&1; then",
        "getent passwd | while IFS=: read -r user _ uid _ _ home _; do",
        r#"{ [ "$uid" -ge 1000 ] 2>/dev/null || [ "$user" = gdm ] || [ "$user" = gdm3 ] || [ "$user" = Debian-gdm ]; } || continue;"#,
        r#"[ -d "$home" ] || continue;"#,
        r#"dconf_profile="";"#,
        r#"case "$user" in gdm|gdm3|Debian-gdm) dconf_profile="DCONF_PROFILE=gdm" ;; esac;"#,
        r#"sudo -u "$user" env HOME="$home" $dconf_profile dbus-run-session sh -c 'EXT_ID="gaze@gundulabs.com";"#,
        "if command -v gsettings >/dev/null 2>&1; then",
        "current=$(gsettings get org.gnome.shell enabled-extensions 2>/dev/null || true);",
        r#"case "$current" in *"$EXT_ID"*)"#,
        r#"next=$(printf "%s" "$current" | sed "s/\047$EXT_ID\047, //; s/, \047$EXT_ID\047//; s/\047$EXT_ID\047//");"#,
        r#"gsettings set org.gnome.shell enabled-extensions "$next" 2>/dev/null || true;;"#,
        "esac;",
        "gsettings reset-recursively org.gnome.shell.extensions.gaze 2>/dev/null || true;",
        "fi;",
        "if command -v dconf >/dev/null 2>&1; then",
        "dconf reset -f /org/gnome/shell/extensions/gaze/ 2>/dev/null || true;",
        "fi' || true;",
        "done; fi",
    ]
    .join(" ")
}

fn remove_gdm_dconf_overrides_cmd() -> String {
    [
        "sudo rm -f /etc/dconf/db/gdm.d/00-gaze-defaults* /etc/dconf/db/gdm.d/99-gaze* &&",
        "if command -v dconf >/dev/null 2>&1; then",
        "sudo dconf update >/dev/null 2>&1 || true;",
        "fi",
    ]
    .join(" ")
}

fn restore_authselect_cmd() -> String {
    [
        "if [ -f /etc/gaze/authselect.previous ]; then",
        r#"profile=$(sudo sed -n 's/^Profile ID:[[:space:]]*//p' /etc/gaze/authselect.previous);"#,
        r#"features=$(sudo sed -n 's/^- //p' /etc/gaze/authselect.previous | tr '\n' ' ');"#,
        r#"if [ -n "$profile" ]; then"#,
        r#"sudo authselect select "$profile" $features --force 2>/dev/null || true;"#,
        "else",
        "sudo authselect select sssd --force 2>/dev/null || true;",
        "fi;",
        "else",
        "sudo authselect select sssd --force 2>/dev/null || true;",
        "fi",
    ]
    .join(" ")
}

fn refresh_gnome_system_settings_cmd() -> String {
    [
        "if command -v dconf >/dev/null 2>&1; then",
        "sudo dconf update >/dev/null 2>&1 || true;",
        "fi;",
        "if command -v glib-compile-schemas >/dev/null 2>&1; then",
        "sudo glib-compile-schemas /usr/share/glib-2.0/schemas >/dev/null 2>&1 || true;",
        "fi",
    ]
    .join(" ")
}

fn remove_unmanaged_install_artifacts_cmd() -> String {
    [
        r#"owned_by_pkg() {
          p=$1;
          if command -v pacman >/dev/null 2>&1; then pacman -Qo "$p" >/dev/null 2>&1 && return 0; fi;
          if command -v dpkg-query >/dev/null 2>&1; then dpkg-query -S "$p" >/dev/null 2>&1 && return 0; fi;
          if command -v rpm >/dev/null 2>&1; then rpm -qf "$p" >/dev/null 2>&1 && return 0; fi;
          return 1;
        };
        remove_if_unmanaged() {
          p=$1;
          [ -e "$p" ] || [ -L "$p" ] || return 0;
          if [ -L "$p" ] || ! owned_by_pkg "$p"; then
            sudo rm -rf "$p";
          fi;
        };
        for p in \
          /usr/bin/gaze /usr/bin/gazed /usr/bin/gaze-gui \
          /usr/local/bin/gaze /usr/local/bin/gazed /usr/local/bin/gaze-gui \
          /usr/lib/security/pam_gaze.so /usr/lib/security/pam_gaze_grosshack.so \
          /usr/lib64/security/pam_gaze.so /usr/lib64/security/pam_gaze_grosshack.so \
          /usr/share/glib-2.0/schemas/org.gnome.shell.extensions.gaze.gschema.xml \
          /usr/share/polkit-1/actions/com.gundulabs.gaze.policy \
          /usr/share/gnome-shell/extensions/gaze@gundulabs.com/extension.js \
          /usr/share/gnome-shell/extensions/gaze@gundulabs.com/metadata.json \
          /usr/share/gnome-shell/extensions/gaze@gundulabs.com/prefs.js
        do remove_if_unmanaged "$p"; done;
        for p in /lib/*/security/pam_gaze.so /lib/*/security/pam_gaze_grosshack.so /usr/lib/*/security/pam_gaze.so /usr/lib/*/security/pam_gaze_grosshack.so; do
          [ -e "$p" ] || [ -L "$p" ] || continue;
          remove_if_unmanaged "$p";
        done;
        sudo rmdir /usr/share/gnome-shell/extensions/gaze@gundulabs.com 2>/dev/null || true;
        sudo rm -rf /etc/systemd/system/gazed.service.d;
        sudo rm -rf /usr/local/share/gaze-dev;
        sudo systemctl daemon-reload >/dev/null 2>&1 || true;
        if command -v glib-compile-schemas >/dev/null 2>&1; then sudo glib-compile-schemas /usr/share/glib-2.0/schemas >/dev/null 2>&1 || true; fi"#,
    ]
    .join(" ")
}

fn remove_arch_pam_configuration_cmd() -> String {
    [
        "for flag in /etc/gaze/pam-arch.configured /etc/gaze/pam-arch.dev-configured; do",
        r#"[ -f "$flag" ] || continue;"#,
        r#"while IFS= read -r f; do"#,
        r#"[ -f "$f" ] || continue;"#,
        r#"sudo sed -i '/pam_gaze/d' "$f" || true;"#,
        "done < \"$flag\";",
        "done;",
        "sudo sed -i '/pam_gaze/d' /etc/pam.d/sudo 2>/dev/null || true;",
        "for flag in /etc/gaze/pam-arch.polkit-configured /etc/gaze/pam-arch.polkit-dev-configured; do",
        r#"[ -f "$flag" ] || continue;"#,
        r#"while IFS= read -r f; do"#,
        r#"sudo rm -f "$f" || true;"#,
        "done < \"$flag\";",
        r#"sudo rm -f "$flag" || true;"#,
        "done",
    ]
    .join(" ")
}

fn remove_rpm_ostree_packages_cmd() -> String {
    "sudo rpm-ostree uninstall gaze gaze-gui gaze-gnome-extension gaze-hyprlock gaze-kde \
      2>/dev/null || \
      for pkg in gaze gaze-gui gaze-gnome-extension gaze-hyprlock gaze-kde; do \
      sudo rpm-ostree uninstall \"$pkg\" 2>/dev/null || true; \
      done"
        .into()
}

fn remove_pacman_packages_cmd() -> String {
    // AUR builds split off `-debug` packages; remove those first since they can
    // depend on the base package.
    "for base in gaze gaze-gui gaze-gnome-extension gaze-hyprlock gaze-kde gaze-bin gaze-gui-bin \
      gaze-gnome-extension-bin gaze-hyprlock-bin gaze-kde-bin; do \
      for pkg in \"$base-debug\" \"$base\"; do \
      if pacman -Q \"$pkg\" >/dev/null 2>&1; then \
      sudo pacman -Rns --noconfirm \"$pkg\" || true; \
      fi; \
      done; \
      done"
        .into()
}

fn remove_zypper_packages_cmd() -> String {
    // Include every optional openSUSE integration package.
    "sudo zypper --non-interactive remove --no-confirm gaze gaze-gui gaze-gnome-extension gaze-hyprlock gaze-kde 2>/dev/null || true"
        .into()
}

fn remove_suse_pam_configuration_cmd() -> String {
    // Remove both managed PAM modes independently.
    "if command -v pam-config >/dev/null 2>&1; then \
      sudo pam-config --delete --gaze 2>/dev/null || true; \
      sudo pam-config --delete --gaze_grosshack 2>/dev/null || true; \
      sudo pam-config --update 2>/dev/null || true; \
      fi"
    .into()
}

const GUNDULABS_REPO_KEY_FINGERPRINT: &str = "505AC1C71AFEDBD5555235F6CB4FA24E5C1C7C98";
// RPM key package versions use the final eight fingerprint characters.

fn remove_zypper_repo_and_key_cmd() -> String {
    // Match only the RPM key package derived from the Gundu Labs fingerprint.
    let key_id = GUNDULABS_REPO_KEY_FINGERPRINT
        .get(GUNDULABS_REPO_KEY_FINGERPRINT.len() - 8..)
        .expect("Gundu Labs fingerprint must contain a key ID")
        .to_ascii_lowercase();
    format!(
        "sudo rm -f /etc/zypp/repos.d/gundulabs.repo \\
          /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs; \\
          if command -v rpm >/dev/null 2>&1; then \\
            rpm -qa 'gpg-pubkey*' --qf '%{{NAME}}-%{{VERSION}}-%{{RELEASE}}\\n' 2>/dev/null | \\
            while IFS= read -r key_package; do \\
              case \"$key_package\" in \\
                gpg-pubkey-{key_id}-*) sudo rpm -e \"$key_package\" 2>/dev/null || true ;; \\
              esac; \\
            done; \\
          fi"
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageManager {
    Apt,
    Zypper,
    Dnf,
    RpmOstree,
    Pacman,
}

fn append_package_manager_uninstall_steps(
    plan: &mut Vec<(&'static str, String)>,
    package_manager: PackageManager,
) {
    match package_manager {
        PackageManager::Apt => {
            plan.push((
                "Remove apt packages",
                "sudo apt-get remove --purge -y gaze gaze-gui gaze-gnome-extension gaze-hyprlock gaze-kde 2>/dev/null || true"
                    .into(),
            ));
            plan.push((
                "Remove apt repo + keyring",
                "sudo rm -f /etc/apt/sources.list.d/gundulabs.list \\
                  /usr/share/keyrings/gundulabs-archive-keyring.gpg && \\
                  sudo apt-get update 2>/dev/null || true"
                    .into(),
            ));
        }
        // Prefer Tumbleweed's native package manager.
        PackageManager::Zypper => {
            plan.push((
                "Remove openSUSE PAM configuration",
                remove_suse_pam_configuration_cmd(),
            ));
            plan.push(("Remove zypper packages", remove_zypper_packages_cmd()));
            plan.push(("Remove zypper repo + key", remove_zypper_repo_and_key_cmd()));
        }
        PackageManager::Dnf => {
            plan.push((
                "Remove dnf packages",
                "sudo dnf remove -y gaze gaze-gui gaze-gnome-extension gaze-hyprlock gaze-kde 2>/dev/null || true"
                    .into(),
            ));
            plan.push((
                "Remove dnf repo",
                "sudo rm -f /etc/yum.repos.d/gundulabs.repo".into(),
            ));
        }
        PackageManager::RpmOstree => {
            plan.push(("Remove layered packages", remove_rpm_ostree_packages_cmd()));
            plan.push((
                "Remove dnf repo",
                "sudo rm -f /etc/yum.repos.d/gundulabs.repo".into(),
            ));
            plan.push((
                "Reboot to finalize removal",
                "echo 'Layered package removal takes effect after the next reboot.'".into(),
            ));
        }
        PackageManager::Pacman => {
            plan.push(("Remove pacman packages", remove_pacman_packages_cmd()));
            plan.push((
                "Remove old pacman repo entry",
                "sudo sed -i '/^\\[gaze\\]/,/^$/d' /etc/pacman.conf && \\
                  sudo rm -f /etc/pacman.d/gaze-mirrorlist"
                    .into(),
            ));
        }
    }
}

fn package_manager_from_availability(
    apt: bool,
    zypper: bool,
    dnf: bool,
    pacman: bool,
    rpm_ostree: bool,
) -> Option<PackageManager> {
    // Prefer zypper when optional apt or dnf tools are also installed.
    if zypper {
        Some(PackageManager::Zypper)
    } else if apt {
        Some(PackageManager::Apt)
    } else if rpm_ostree {
        Some(PackageManager::RpmOstree)
    } else if dnf {
        Some(PackageManager::Dnf)
    } else if pacman {
        Some(PackageManager::Pacman)
    } else {
        None
    }
}

fn detect_package_manager() -> Option<PackageManager> {
    package_manager_from_availability(
        which("apt-get"),
        which("zypper"),
        which("dnf"),
        which("pacman"),
        which("rpm-ostree") && std::path::Path::new("/run/ostree-booted").exists(),
    )
}

fn build_uninstall_plan(keep_data: bool) -> Vec<(&'static str, String)> {
    let mut plan: Vec<(&'static str, String)> = Vec::new();

    if which("gnome-extensions") {
        plan.push((
            "Disable and uninstall GNOME extension (best-effort)",
            "gnome-extensions disable gaze@gundulabs.com 2>/dev/null || true; \
              gnome-extensions uninstall gaze@gundulabs.com 2>/dev/null || true"
                .into(),
        ));
    }

    plan.push((
        "Reset GNOME lock/login settings",
        reset_gnome_user_settings_cmd(),
    ));
    plan.push((
        "Remove per-user GNOME extension copies",
        "for d in /home/*/.local/share/gnome-shell/extensions /root/.local/share/gnome-shell/extensions; do \
          [ -d \"$d/gaze@gundulabs.com\" ] || continue; \
          sudo rm -rf \"$d/gaze@gundulabs.com\"; \
          done"
            .into(),
    ));
    plan.push((
        "Remove GDM dconf overrides",
        remove_gdm_dconf_overrides_cmd(),
    ));

    if which("pam-auth-update") {
        plan.push((
            "Remove Debian/Ubuntu PAM profile",
            "sudo pam-auth-update --package --remove gaze 2>/dev/null || true".into(),
        ));
    }
    if which("authselect") {
        plan.push(("Restore authselect profile", restore_authselect_cmd()));
    }

    if which("pacman") && !which("pam-auth-update") && !which("authselect") {
        plan.push((
            "Remove Arch PAM configuration",
            remove_arch_pam_configuration_cmd(),
        ));
    }

    plan.push((
        "Remove hyprlock Gaze PAM references",
        "for d in /home/*/.config/hypr /root/.config/hypr; do \
          f=\"$d/hyprlock.conf\"; \
          [ -f \"$f\" ] || continue; \
          sudo sed -i.gaze-uninstall-bak \
            '/^\\s*\\(pam_\\)\\?module\\s*=\\s*hyprlock-gaze\\(-simultaneous\\)\\?\\s*$/d' \"$f\" || true; \
          done"
            .into(),
    ));

    plan.push((
        "Stop and disable daemon",
        "sudo systemctl disable --now gazed 2>/dev/null || true".into(),
    ));

    if let Some(package_manager) = detect_package_manager() {
        append_package_manager_uninstall_steps(&mut plan, package_manager);
    }

    if which("semodule") {
        plan.push((
            "Remove SELinux policy",
            "sudo semodule -r gaze-gdm-camera 2>/dev/null || true".into(),
        ));
    }

    plan.push((
        "Remove unmanaged development links/files",
        remove_unmanaged_install_artifacts_cmd(),
    ));

    plan.push((
        // gazed holds decrypted face templates in memory, so its crash dumps
        // are biometric data too.
        "Remove gaze core dumps",
        "[ -d /var/lib/systemd/coredump ] && \
          sudo find /var/lib/systemd/coredump \\( -name 'core.gazed.*' \
          -o -name 'core.gaze.*' -o -name 'core.gaze-gui.*' \\) -delete \
          2>/dev/null || true"
            .into(),
    ));
    plan.push(("Remove model cache", "sudo rm -rf /var/cache/gaze".into()));
    plan.push(("Remove config", "sudo rm -rf /etc/gaze".into()));
    if !keep_data {
        plan.push((
            "Remove enrolled face data",
            "sudo rm -rf /var/lib/gaze".into(),
        ));
    }

    plan.push((
        "Refresh GNOME system settings",
        refresh_gnome_system_settings_cmd(),
    ));
    plan.push(("Reload systemd", "sudo systemctl daemon-reload".into()));

    plan
}

fn handle_uninstall(yes: bool, keep_data: bool, dry_run: bool) -> anyhow::Result<()> {
    let term = Term::stdout();
    let plan = build_uninstall_plan(keep_data);

    term.write_line(&format!(
        "\n{}\n",
        style("Gaze uninstall plan").red().bold()
    ))?;
    for (i, (desc, cmd)) in plan.iter().enumerate() {
        term.write_line(&format!(
            "  {} {}\n    {}",
            style(format!("{:>2}.", i + 1)).dim(),
            style(desc).bold(),
            style(cmd).dim()
        ))?;
    }
    term.write_line("")?;

    if keep_data {
        term.write_line(&format!(
            "  {} /var/lib/gaze (enrolled faces) will be preserved.",
            style("i").cyan().bold()
        ))?;
    } else {
        term.write_line(&format!(
            "  {} This removes enrolled face data. Pass --keep-data to preserve it.",
            style("!").yellow().bold()
        ))?;
    }
    term.write_line("")?;

    if dry_run {
        term.write_line(&format!(
            "{} Dry run; no commands were executed.",
            style("i").cyan().bold()
        ))?;
        return Ok(());
    }

    if !yes {
        let theme = ColorfulTheme::default();
        let proceed = Select::with_theme(&theme)
            .with_prompt("Proceed with uninstall?")
            .items(["No, cancel", "Yes, uninstall Gaze"])
            .default(0)
            .interact()?;
        if proceed != 1 {
            term.write_line(&format!("{} Cancelled.", style("✗").red().bold()))?;
            return Ok(());
        }
    }

    for (desc, cmd) in &plan {
        term.write_line(&format!("\n{} {}", style("▶").cyan().bold(), desc))?;
        let status = std::process::Command::new("sh").arg("-c").arg(cmd).status();
        match status {
            Ok(s) if s.success() => {
                term.write_line(&format!("  {} done", style("✓").green()))?;
            }
            Ok(s) => {
                term.write_line(&format!(
                    "  {} step exited with {} (continuing)",
                    style("!").yellow(),
                    s.code().unwrap_or(-1)
                ))?;
            }
            Err(e) => {
                term.write_line(&format!(
                    "  {} failed to spawn: {} (continuing)",
                    style("!").yellow(),
                    e
                ))?;
            }
        }
    }

    term.write_line(&format!(
        "\n{} Gaze uninstalled. A reboot is recommended to clear any in-memory state.",
        style("✓").green().bold()
    ))?;
    term.write_line(&format!(
        "  {} If a hyprlock.conf referenced Gaze, a backup was left next to it \
          as hyprlock.conf.gaze-uninstall-bak.",
        style("i").cyan().bold()
    ))?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    CompleteEnv::with_factory(Cli::command).complete();

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(name) = command_requires_root(&cli.command)
        && !is_root()
    {
        reexec_as_root(name)?;
    }

    let silent_auth = matches!(cli.command, Commands::Auth { silent: true, .. });

    let _polkit_agent =
        (command_may_be_challenged(&cli.command) && !silent_auth).then(polkit::PolkitAgent::spawn);

    match &cli.command {
        Commands::Uninstall {
            yes,
            keep_data,
            dry_run,
        } => return handle_uninstall(*yes, *keep_data, *dry_run),
        Commands::Doctor { user, benchmark } => {
            let username = user.clone().unwrap_or_else(get_current_user);
            let healthy = doctor::run(&username, *benchmark).await?;
            if !healthy {
                std::process::exit(1);
            }
            return Ok(());
        }
        _ => {}
    }

    let proxy = match connect_gaze().await {
        Ok(proxy) => proxy,
        Err(_) if silent_auth => std::process::exit(1),
        Err(e) => return Err(e.into()),
    };

    match cli.command {
        Commands::Auth {
            user,
            verbose,
            silent,
        } => {
            let result = handle_auth(
                &proxy,
                &user.unwrap_or_else(get_current_user),
                verbose,
                silent,
            )
            .await;
            if silent && result.is_err() {
                std::process::exit(1);
            }
            result?;
        }
        Commands::AddFace { user, face } => {
            handle_enroll(&proxy, &user.unwrap_or_else(get_current_user), &face, false).await?;
        }
        Commands::RefineFace { user, face } => {
            handle_enroll(&proxy, &user.unwrap_or_else(get_current_user), &face, true).await?;
        }
        Commands::ListFaces { user } => {
            handle_list_faces(&proxy, &user.unwrap_or_else(get_current_user)).await?;
        }
        Commands::RemoveFace { user, face } => {
            handle_remove_face(&proxy, &user.unwrap_or_else(get_current_user), &face).await?;
        }
        Commands::RenameFace { user, from, to } => {
            handle_rename_face(&proxy, &user.unwrap_or_else(get_current_user), &from, &to).await?;
        }
        Commands::ClearUser { user } => {
            handle_clear_user(&proxy, &user.unwrap_or_else(get_current_user)).await?;
        }
        Commands::Config { show } => {
            let config = load_config_from_daemon(&proxy).await?;
            if show {
                println!(
                    "{} {}",
                    style("inference.execution_provider:").bold(),
                    config.inference.execution_provider
                );
                println!(
                    "{} {}",
                    style("inference.device:").bold(),
                    config.inference.device
                );
                let level_name = config.security.level.as_str();
                println!("{} {}", style("security.level:").bold(), level_name);
                println!(
                    "{} {}",
                    style("security.detector:").bold(),
                    config.security.detector()
                );
                println!(
                    "{} {}",
                    style("security.recognizer:").bold(),
                    config.security.recognizer()
                );
                println!(
                    "{} {:.2}",
                    style("security.rgb_threshold:").bold(),
                    config.security.rgb_threshold()
                );
                println!(
                    "{} {:.2}",
                    style("security.ir_threshold:").bold(),
                    config.security.ir_threshold()
                );
                println!(
                    "{} {}",
                    style("security.hybrid_policy:").bold(),
                    if config.security.hybrid_policy.is_empty() {
                        format!("\"\" (resolved: {})", config.security.hybrid_policy())
                    } else {
                        config.security.hybrid_policy.clone()
                    }
                );
                println!("{} {}", style("cameras.rgb:").bold(), config.cameras.rgb);
                println!("{} {}", style("cameras.ir:").bold(), config.cameras.ir);
                println!(
                    "{} {}",
                    style("cameras.emitter_enabled:").bold(),
                    config.cameras.emitter_enabled
                );
                println!(
                    "{} {}",
                    style("cameras.dark_luma_threshold:").bold(),
                    config.cameras.dark_luma_threshold
                );
                println!(
                    "{} {}",
                    style("cameras.parallel_capture:").bold(),
                    config.cameras.parallel_capture()
                );
                println!(
                    "{} {}",
                    style("auth.abort_if_ssh:").bold(),
                    config.auth.abort_if_ssh
                );
                println!(
                    "{} {}",
                    style("auth.abort_if_lid_closed:").bold(),
                    config.auth.abort_if_lid_closed
                );
                println!(
                    "{} {}",
                    style("auth.abort_before_first_resume:").bold(),
                    config.auth.abort_before_first_resume
                );
                println!(
                    "{} {}",
                    style("auth.require_confirmation_lock_screen:").bold(),
                    config.auth.require_confirmation_lock_screen
                );
                println!(
                    "{} {}",
                    style("auth.require_confirmation_elevation:").bold(),
                    config.auth.require_confirmation_elevation
                );
                println!(
                    "{} {}",
                    style("auth.resume_grace_ms:").bold(),
                    config.auth.resume_grace_ms
                );
                println!(
                    "{} {}",
                    style("auth.start_delay_ms:").bold(),
                    config.auth.start_delay_ms
                );
                println!(
                    "{} {}",
                    style("auth.start_delay_scope:").bold(),
                    config.auth.start_delay_scope()
                );

                println!(
                    "{} {}",
                    style("enrollment.max_templates:").bold(),
                    config.enrollment.max_templates
                );
                println!(
                    "{} {:.2}",
                    style("enrollment.min_face_size_ratio:").bold(),
                    config.enrollment.min_face_size_ratio
                );
                println!(
                    "{} {}",
                    style("liveness.enabled:").bold(),
                    config.liveness.enabled
                );
                println!(
                    "{} {:.2}",
                    style("liveness.threshold:").bold(),
                    config.liveness.threshold
                );
                println!(
                    "{} {}",
                    style("liveness.max_frames:").bold(),
                    config.liveness.max_frames
                );
                println!(
                    "{} {}",
                    style("storage.encrypt_templates:").bold(),
                    config.storage.encrypt_templates
                );
                return Ok(());
            }
            run_config_wizard(&Term::stdout(), &proxy, config).await?;
        }

        Commands::Doctor { .. } | Commands::Uninstall { .. } => {
            unreachable!("handled before DBus connection")
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_has(plan: &[(&'static str, String)], label: &str) -> bool {
        plan.iter().any(|(candidate, _)| *candidate == label)
    }

    #[test]
    fn cli_parses_auth_and_safe_uninstall_flags() {
        let cli = Cli::try_parse_from(["gaze", "auth", "--user", "alice", "--verbose"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Auth {
                user: Some(ref user),
                verbose: true,
                silent: false,
            } if user == "alice"
        ));

        let cli = Cli::try_parse_from(["gaze", "auth", "-s", "-u", "bob"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Auth {
                user: Some(ref user),
                verbose: false,
                silent: true,
            } if user == "bob"
        ));

        let cli = Cli::try_parse_from(["gaze", "auth", "--silent"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Auth {
                user: None,
                verbose: false,
                silent: true,
            }
        ));

        assert!(Cli::try_parse_from(["gaze", "auth", "--verbose", "--silent"]).is_err());

        let cli = Cli::try_parse_from(["gaze", "uninstall", "--yes", "--keep-data", "--dry-run"])
            .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Uninstall {
                yes: true,
                keep_data: true,
                dry_run: true
            }
        ));

        let cli = Cli::try_parse_from(["gaze", "doctor", "--user", "alice"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Doctor {
                user: Some(ref user),
                benchmark: false,
            } if user == "alice"
        ));

        let cli = Cli::try_parse_from(["gaze", "doctor", "--benchmark"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Doctor {
                benchmark: true,
                ..
            }
        ));
    }

    #[test]
    fn face_and_config_writes_require_root() {
        for (args, expected) in [
            (vec!["gaze", "add-face", "default"], "add-face"),
            (vec!["gaze", "refine-face", "default"], "refine-face"),
            (vec!["gaze", "remove-face", "default"], "remove-face"),
            (vec!["gaze", "rename-face", "old", "new"], "rename-face"),
            (vec!["gaze", "clear-user"], "clear-user"),
            (vec!["gaze", "config"], "config"),
        ] {
            let cli = Cli::try_parse_from(&args).unwrap();
            assert_eq!(
                command_requires_root(&cli.command),
                Some(expected),
                "{args:?} must require root"
            );
        }
    }

    #[test]
    fn read_only_commands_stay_unprivileged() {
        for args in [
            vec!["gaze", "auth"],
            vec!["gaze", "list-faces"],
            vec!["gaze", "doctor"],
            vec!["gaze", "config", "--show"],
        ] {
            let cli = Cli::try_parse_from(&args).unwrap();
            assert_eq!(
                command_requires_root(&cli.command),
                None,
                "{args:?} must stay unprivileged"
            );
        }
    }

    #[test]
    fn only_cross_user_reads_need_a_tty_polkit_agent() {
        let cli = Cli::try_parse_from(["gaze", "list-faces", "--user", "alice"]).unwrap();
        assert_eq!(command_target_user(&cli.command), Some("alice"));

        let cli = Cli::try_parse_from(["gaze", "doctor", "--user", "alice"]).unwrap();
        assert_eq!(command_target_user(&cli.command), Some("alice"));

        let cli = Cli::try_parse_from(["gaze", "auth"]).unwrap();
        assert_eq!(command_target_user(&cli.command), None);

        let cli = Cli::try_parse_from(["gaze", "add-face", "default"]).unwrap();
        assert_eq!(command_target_user(&cli.command), None);
    }

    #[test]
    fn resolve_current_user_prefers_the_account_behind_sudo() {
        assert_eq!(
            resolve_current_user(Some("alice".into()), Some("root".into())),
            "alice"
        );
        assert_eq!(resolve_current_user(None, Some("alice".into())), "alice");
        assert_eq!(
            resolve_current_user(Some(String::new()), Some("alice".into())),
            "alice"
        );
        assert_eq!(resolve_current_user(None, None), "root");
    }

    #[test]
    fn uninstall_plan_preserves_face_data_only_when_requested() {
        assert!(plan_has(
            &build_uninstall_plan(false),
            "Remove enrolled face data"
        ));
        assert!(!plan_has(
            &build_uninstall_plan(true),
            "Remove enrolled face data"
        ));
    }

    #[test]
    fn uninstall_always_removes_unmanaged_development_artifacts() {
        let plan = build_uninstall_plan(true);
        assert!(plan_has(&plan, "Remove unmanaged development links/files"));

        let command = remove_unmanaged_install_artifacts_cmd();
        for path in [
            "/usr/bin/gaze",
            "/usr/local/bin/gazed",
            "/usr/lib/security/pam_gaze.so",
            "/usr/share/gnome-shell/extensions/gaze@gundulabs.com/extension.js",
            "/usr/share/polkit-1/actions/com.gundulabs.gaze.policy",
            "/usr/local/share/gaze-dev",
        ] {
            assert!(command.contains(path), "missing cleanup for {path}");
        }
        assert!(command.contains("[ -L \"$p\" ] || ! owned_by_pkg \"$p\""));
        assert!(command.contains("sudo rm -rf /etc/systemd/system/gazed.service.d"));
    }

    #[test]
    fn uninstall_plan_removes_per_user_extensions_and_core_dumps() {
        let plan = build_uninstall_plan(true);
        assert!(plan_has(&plan, "Remove per-user GNOME extension copies"));
        assert!(plan_has(&plan, "Remove gaze core dumps"));

        let (_, cmd) = plan
            .iter()
            .find(|(desc, _)| *desc == "Remove per-user GNOME extension copies")
            .unwrap();
        assert!(cmd.contains("/home/*/.local/share/gnome-shell/extensions"));
        assert!(cmd.contains("/root/.local/share/gnome-shell/extensions"));

        let (_, cmd) = plan
            .iter()
            .find(|(desc, _)| *desc == "Remove gaze core dumps")
            .unwrap();
        assert!(cmd.contains("/var/lib/systemd/coredump"));
    }

    #[test]
    fn pacman_removal_covers_debug_split_packages() {
        let command = remove_pacman_packages_cmd();
        assert!(command.contains("gaze-bin"));
        assert!(command.contains("\"$base-debug\" \"$base\""));

        let output = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(&command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "invalid pacman removal shell command: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn zypper_removal_covers_all_native_packages() {
        let command = remove_zypper_packages_cmd();
        for package in [
            "gaze",
            "gaze-gui",
            "gaze-gnome-extension",
            "gaze-hyprlock",
            "gaze-kde",
        ] {
            assert!(
                command.contains(package),
                "missing zypper removal for {package}"
            );
        }
        assert!(command.contains("zypper --non-interactive remove --no-confirm"));

        let output = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(&command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "invalid zypper removal shell command: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn suse_uninstall_removes_both_pam_config_definitions() {
        let command = remove_suse_pam_configuration_cmd();
        assert!(command.contains("pam-config --delete --gaze"));
        assert!(command.contains("pam-config --delete --gaze_grosshack"));
        assert!(command.contains("pam-config --update"));

        let output = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(&command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "invalid openSUSE PAM cleanup shell command: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn suse_uninstall_branch_removes_packages_pam_repo_and_imported_key() {
        let mut plan = Vec::new();
        append_package_manager_uninstall_steps(&mut plan, PackageManager::Zypper);

        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].0, "Remove openSUSE PAM configuration");
        assert_eq!(plan[1].0, "Remove zypper packages");
        assert_eq!(plan[2].0, "Remove zypper repo + key");

        let command = &plan[2].1;
        assert!(command.contains("/etc/zypp/repos.d/gundulabs.repo"));
        assert!(command.contains("/etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs"));
        assert!(command.contains("gpg-pubkey-5c1c7c98-*)"));
        assert!(command.contains("sudo rpm -e \"$key_package\""));
        assert_eq!(
            GUNDULABS_REPO_KEY_FINGERPRINT,
            "505AC1C71AFEDBD5555235F6CB4FA24E5C1C7C98"
        );
        assert!(!command.contains("rpm -e gpg-pubkey*"));

        let output = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "invalid openSUSE repo/key cleanup shell command: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn open_suse_package_manager_precedes_dnf_when_both_are_available() {
        assert_eq!(
            package_manager_from_availability(false, true, true, false, false),
            Some(PackageManager::Zypper)
        );
        assert_eq!(
            package_manager_from_availability(true, true, true, false, false),
            Some(PackageManager::Zypper)
        );
    }

    #[test]
    fn rpm_ostree_package_manager_precedes_dnf_when_both_are_available() {
        assert_eq!(
            package_manager_from_availability(false, false, true, false, true),
            Some(PackageManager::RpmOstree)
        );
        assert_eq!(
            package_manager_from_availability(false, false, true, false, false),
            Some(PackageManager::Dnf)
        );
    }

    #[test]
    fn rpm_ostree_package_cleanup_is_valid_shell() {
        let command = remove_rpm_ostree_packages_cmd();
        let output = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(&command)
            .output()
            .expect("failed to run sh -n");
        assert!(
            output.status.success(),
            "invalid rpm-ostree cleanup shell command: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn unmanaged_development_artifact_cleanup_is_valid_shell() {
        let command = remove_unmanaged_install_artifacts_cmd();
        let output = std::process::Command::new("sh")
            .arg("-n")
            .arg("-c")
            .arg(&command)
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "invalid cleanup shell command: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn arch_pam_cleanup_handles_package_and_dev_link_markers() {
        let command = remove_arch_pam_configuration_cmd();
        assert!(command.contains("/etc/gaze/pam-arch.configured"));
        assert!(command.contains("/etc/gaze/pam-arch.dev-configured"));
        assert!(command.contains("sed -i '/pam_gaze/d'"));
        assert!(command.contains("/etc/pam.d/sudo"));
        assert!(command.contains("/etc/gaze/pam-arch.polkit-configured"));
        assert!(command.contains("/etc/gaze/pam-arch.polkit-dev-configured"));
        assert!(command.contains("rm -f"));
    }
}
