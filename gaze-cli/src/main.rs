mod tui;

use clap::{Parser, Subcommand};
use console::{Term, style};
use dialoguer::{Confirm, Input, Select, theme::ColorfulTheme};
use futures::StreamExt;
use gaze_core::config::{
    Config, HYBRID_POLICY_OPTIONS, MODEL_QUALITY_OPTIONS, SECURITY_LEVEL_OPTIONS, SecurityLevel,
};
use gaze_core::dbus::{
    CaptureStatus, EnrollPrompt, GazeProxy, VerifyResult, apply_config_to_daemon, connect_gaze,
    dbus_error_message, dbus_is_file_not_found, load_config_from_daemon,
};
use std::{future::Future, time::Duration};
use tui::{AuthScreen, BusyScreen, EnrollScreen, Tone, TuiAction, TuiTerminal};

fn get_current_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".into())
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

async fn run_busy<F, T>(title: &str, message: String, tone: Tone, future: F) -> anyhow::Result<T>
where
    F: Future<Output = T>,
{
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
#[command(name = "gaze", version, about = "Gaze Facial Authentication CLI")]
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
        #[arg(short, long, help = "Show detailed authentication metrics")]
        verbose: bool,
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
        #[arg(help = "The name of the face to refine")]
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
        #[arg(help = "The name of the face to remove")]
        face: String,
    },
    /// Rename a face for a user
    RenameFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "Current face name")]
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
        let (old_detector, old_recognizer, old_threshold, old_hybrid_policy) =
            if config.security.level == "custom" {
                (
                    config.security.detector.clone(),
                    config.security.recognizer.clone(),
                    config.security.threshold,
                    config.security.hybrid_policy.clone(),
                )
            } else {
                (
                    "accurate".to_string(),
                    "accurate".to_string(),
                    0.6,
                    String::new(),
                )
            };

        let selected_det_idx = Select::with_theme(&theme)
            .with_prompt("Custom detector level")
            .items(MODEL_QUALITY_OPTIONS)
            .default(SecurityLevel::model_quality_index(&old_detector) as usize)
            .interact()?;
        let detector = SecurityLevel::model_quality_from_index(selected_det_idx).to_string();

        let selected_rec_idx = Select::with_theme(&theme)
            .with_prompt("Custom recognizer level")
            .items(MODEL_QUALITY_OPTIONS)
            .default(SecurityLevel::model_quality_index(&old_recognizer) as usize)
            .interact()?;
        let recognizer = SecurityLevel::model_quality_from_index(selected_rec_idx).to_string();

        let threshold = Input::with_theme(&theme)
            .with_prompt("Custom threshold (0.0 - 1.0)")
            .default(old_threshold.to_string())
            .interact_text()?
            .parse::<f64>()
            .unwrap_or(0.6);

        let selected_hybrid_idx = Select::with_theme(&theme)
            .with_prompt("Custom hybrid combining policy")
            .items(HYBRID_POLICY_OPTIONS)
            .default(SecurityLevel::hybrid_policy_index_for_value(&old_hybrid_policy) as usize)
            .interact()?;
        let hybrid_policy = SecurityLevel::hybrid_policy_from_index(selected_hybrid_idx);

        config.security = SecurityLevel::custom(detector, recognizer, threshold, hybrid_policy);
    };

    let cameras = gaze_core::camera::enumerate_cameras().unwrap_or_default();
    if cameras.is_empty() {
        anyhow::bail!("No PipeWire cameras detected! Please ensure your video inputs are active.");
    }
    let cam_names: Vec<String> = cameras.iter().map(|(n, _)| n.clone()).collect();
    let default_cam_idx = cameras
        .iter()
        .position(|(_, target)| target == &config.cameras.rgb)
        .unwrap_or(0);

    let selected_cam_idx = Select::with_theme(&theme)
        .with_prompt("RGB camera source")
        .items(&cam_names)
        .default(default_cam_idx)
        .interact()?;

    config.cameras.rgb = cameras[selected_cam_idx].1.clone();

    config.cameras.dark_luma_threshold = Input::with_theme(&theme)
        .with_prompt("Darkness cutoff: reject frames below this mean brightness (0-255)")
        .default(config.cameras.dark_luma_threshold.to_string())
        .interact_text()?
        .parse::<u8>()
        .unwrap_or(30);

    let ir_cameras = gaze_core::camera::enumerate_ir_cameras().unwrap_or_default();
    let mut ir_options = vec![("None".to_string(), String::new())];
    ir_options.extend(ir_cameras);

    let ir_names: Vec<String> = ir_options.iter().map(|(n, _)| n.clone()).collect();
    let default_ir_idx = ir_options
        .iter()
        .position(|(_, target)| target == &config.cameras.ir)
        .unwrap_or(0);

    let selected_ir_idx = Select::with_theme(&theme)
        .with_prompt("IR camera source")
        .items(&ir_names)
        .default(default_ir_idx)
        .interact()?;

    config.cameras.ir = ir_options[selected_ir_idx].1.clone();

    if config.cameras.ir.is_empty() {
        config.cameras.emitter_enabled = false;
    } else {
        config.cameras.emitter_enabled = Confirm::with_theme(&theme)
            .with_prompt("Force IR emitter override (only use if emitter stays off automatically)")
            .default(config.cameras.emitter_enabled)
            .interact()?;
    }

    config.auth.abort_if_ssh = Confirm::with_theme(&theme)
        .with_prompt("Abort face auth for SSH sessions")
        .default(config.auth.abort_if_ssh)
        .interact()?;

    config.auth.abort_if_lid_closed = Confirm::with_theme(&theme)
        .with_prompt("Abort face auth when laptop lid is closed")
        .default(config.auth.abort_if_lid_closed)
        .interact()?;

    config.auth.require_confirmation = Confirm::with_theme(&theme)
        .with_prompt(
            "Require confirmation (press Enter/Authenticate/OK) to authorize after face matches",
        )
        .default(config.auth.require_confirmation)
        .interact()?;

    config.enrollment.max_templates = Input::with_theme(&theme)
        .with_prompt("Max templates (sets of captures)")
        .default(config.enrollment.max_templates)
        .interact_text()?;

    config.liveness.enabled = Confirm::with_theme(&theme)
        .with_prompt("Enable liveness anti-spoofing")
        .default(config.liveness.enabled)
        .interact()?;
    if config.liveness.enabled {
        config.liveness.threshold = Input::with_theme(&theme)
            .with_prompt("Liveness threshold (0.0 - 1.0)")
            .default(config.liveness.threshold.to_string())
            .interact_text()?
            .parse::<f64>()
            .unwrap_or(0.8);
        config.liveness.max_frames = Input::with_theme(&theme)
            .with_prompt("Liveness max frames")
            .default(config.liveness.max_frames)
            .interact_text()?;
    }

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
        return Ok(());
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

                        if matches!(raw_msg, EnrollPrompt::DbFailed | EnrollPrompt::Cancelled) {
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
        term.write_line(&format!("{} Enrollment failed", style("✗").red().bold()))?;
    }
    Ok(())
}

async fn handle_auth(proxy: &GazeProxy<'_>, user: &str, verbose: bool) -> anyhow::Result<()> {
    let term = Term::stdout();
    let start = std::time::Instant::now();

    if let Err(err) = proxy.claim(user).await {
        term.write_line(&format!(
            "{} Failed to claim device: {}",
            style("✗").red().bold(),
            dbus_error_message(&err)
        ))?;
        return Ok(());
    }

    let mut status_stream = proxy.receive_verify_status().await?;
    let mut capture_stream = proxy.receive_face_status().await?;
    let mut terminal = match TuiTerminal::new() {
        Ok(terminal) => terminal,
        Err(err) => {
            let _ = proxy.release().await;
            return Err(err);
        }
    };
    if let Err(e) = proxy.verify_start("any").await {
        drop(terminal);
        term.write_line(&format!("{} Daemon error: {}", style("✗").red().bold(), e))?;
        let _ = proxy.release().await;
        return Ok(());
    }

    let mut status_msg = format!("Scanning face for {user}...");
    let mut status_tone = Tone::Info;
    let mut tick = 0_u64;
    let mut cancelled = false;
    let mut verify_result = None;

    loop {
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

        tokio::select! {
            signal = status_stream.next() => {
                if let Some(signal) = signal
                    && let Ok(args) = signal.args()
                {
                    verify_result = Some((*args.result(), args.faces().clone(), *args.rgb_status(), *args.ir_status()));
                    break;
                }
            }
            signal = capture_stream.next() => {
                if let Some(signal) = signal
                    && let Ok(args) = signal.args()
                {
                    let status = *args.status();
                    status_tone = capture_tone(status);
                    status_msg = match status {
                        CaptureStatus::Ready | CaptureStatus::Usable => format!("Scanning face for {user}..."),
                        _ => status.to_string(),
                    };
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(80)) => {
                tick = tick.wrapping_add(1);
            }
        }
    }

    drop(terminal);

    if cancelled {
        let _ = proxy.verify_stop().await;
        let _ = proxy.release().await;
        std::process::exit(130);
    }

    if let Some((result, faces, rgb_status, ir_status)) = verify_result {
        if verbose {
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
        } else {
            term.write_line(&format!(
                "{} Authentication failed ({}ms)",
                style("✗").red().bold(),
                start.elapsed().as_millis()
            ))?;
        }
    }
    let _ = proxy.release().await;
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
                    "\n{} faces for {}:\n",
                    style(faces.len()).green().bold(),
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
                        "  {} {} {} {} ({} captures)",
                        style("•").cyan(),
                        style(face).bold(),
                        rgb_badge,
                        ir_badge,
                        count
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

fn build_uninstall_plan(keep_data: bool) -> Vec<(&'static str, String)> {
    let mut plan: Vec<(&'static str, String)> = Vec::new();

    if which("gnome-extensions") {
        plan.push((
            "Disable GNOME extension (best-effort)",
            "gnome-extensions disable gaze@gundulabs.com 2>/dev/null || true".into(),
        ));
    }

    plan.push((
        "Reset GNOME lock/login settings",
        reset_gnome_user_settings_cmd(),
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

    plan.push((
        "Remove hyprlock pam_module references",
        "for d in /home/*/.config/hypr /root/.config/hypr; do \
          f=\"$d/hyprlock.conf\"; \
          [ -f \"$f\" ] || continue; \
          sudo sed -i.gaze-uninstall-bak \
            '/^\\s*pam_module\\s*=\\s*hyprlock-gaze\\(-simultaneous\\)\\?\\s*$/d' \"$f\" || true; \
          done"
            .into(),
    ));

    plan.push((
        "Stop and disable daemon",
        "sudo systemctl disable --now gazed 2>/dev/null || true".into(),
    ));

    if which("apt-get") {
        plan.push((
            "Remove apt packages",
            "sudo apt-get remove --purge -y gaze gaze-gui gaze-gnome-extension gaze-hyprlock 2>/dev/null || true"
                .into(),
        ));
        plan.push((
            "Remove apt repo + keyring",
            "sudo rm -f /etc/apt/sources.list.d/gundulabs.list \
              /usr/share/keyrings/gundulabs-archive-keyring.gpg && \
              sudo apt-get update 2>/dev/null || true"
                .into(),
        ));
    } else if which("dnf") {
        plan.push((
            "Remove dnf packages",
            "sudo dnf remove -y gaze gaze-gui gaze-gnome-extension gaze-hyprlock 2>/dev/null || true".into(),
        ));
        plan.push((
            "Remove dnf repo",
            "sudo rm -f /etc/yum.repos.d/gundulabs.repo".into(),
        ));
    } else if which("pacman") {
        plan.push((
            "Remove pacman packages",
            "for pkg in gaze gaze-gui gaze-gnome-extension gaze-hyprlock gaze-bin gaze-gui-bin \
              gaze-gnome-extension-bin gaze-hyprlock-bin; do \
              if pacman -Q \"$pkg\" >/dev/null 2>&1; then \
              sudo pacman -Rns --noconfirm \"$pkg\" || true; \
              fi; \
              done"
                .into(),
        ));
        plan.push((
            "Remove old pacman repo entry",
            "sudo sed -i '/^\\[gaze\\]/,/^$/d' /etc/pacman.conf && \
              sudo rm -f /etc/pacman.d/gaze-mirrorlist"
                .into(),
        ));
    }

    if which("semodule") {
        plan.push((
            "Remove SELinux policy",
            "sudo semodule -r gaze-gdm-camera 2>/dev/null || true".into(),
        ));
    }

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
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Commands::Uninstall {
        yes,
        keep_data,
        dry_run,
    } = cli.command
    {
        return handle_uninstall(yes, keep_data, dry_run);
    }

    let proxy = connect_gaze().await?;

    match cli.command {
        Commands::Auth { user, verbose } => {
            handle_auth(&proxy, &user.unwrap_or_else(get_current_user), verbose).await?;
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
                    style("security.threshold:").bold(),
                    config.security.threshold()
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
                    style("auth.require_confirmation:").bold(),
                    config.auth.require_confirmation
                );

                println!(
                    "{} {}",
                    style("enrollment.max_templates:").bold(),
                    config.enrollment.max_templates
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
                return Ok(());
            }
            run_config_wizard(&Term::stdout(), &proxy, config).await?;
        }

        Commands::Uninstall { .. } => unreachable!("handled before DBus connection"),
    }

    Ok(())
}
