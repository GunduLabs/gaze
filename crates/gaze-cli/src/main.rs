// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

mod configure;
mod doctor;
mod faces;
mod keyring;
mod polkit;
mod selinux;
mod tui;
mod uninstall;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::CompleteEnv;
use clap_complete::engine::{ArgValueCompleter, CompletionCandidate};
use configure::{config_show_text, run_config_wizard};
use console::{Term, style};
use faces::{
    handle_clear_user, handle_duress, handle_list_faces, handle_remove_face, handle_rename_face,
};
use futures::StreamExt;
use gaze_core::config::Config;
use gaze_core::dbus::{
    CaptureStatus, EnrollPrompt, GazeProxy, VerifyResult, apply_config_to_daemon,
    apply_config_with_keyring_to_daemon, connect_gaze, daemon_unavailable_hint, dbus_error_message,
    dbus_is_not_activatable, load_config_with_keyring_from_daemon,
};
use std::{future::Future, time::Duration};
use tui::{AuthScreen, BusyScreen, EnrollScreen, Tone, TuiAction, TuiTerminal};
use uninstall::{handle_uninstall, which};

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
        Commands::Config { show, action } => match action {
            Some(ConfigAction::Get { .. }) => None,
            Some(ConfigAction::Set { .. }) => Some("config"),
            None => (!show).then_some("config"),
        },
        Commands::Keyring { .. } => Some("keyring"),
        Commands::Auth { .. }
        | Commands::ListFaces { .. }
        | Commands::Duress { .. }
        | Commands::Doctor { .. }
        | Commands::Uninstall { .. } => None,
    }
}

fn command_target_user(command: &Commands) -> Option<&str> {
    match command {
        Commands::Auth { user, .. }
        | Commands::ListFaces { user, .. }
        | Commands::Duress { user, .. }
        | Commands::Doctor { user, .. } => user.as_deref(),
        _ => None,
    }
}

fn command_may_be_challenged(command: &Commands) -> bool {
    command_may_be_challenged_as(command, is_root(), &get_current_user())
}

fn command_may_be_challenged_as(command: &Commands, root: bool, current_user: &str) -> bool {
    !root
        && (matches!(command, Commands::Duress { clear: true, .. })
            || matches!(command_target_user(command), Some(user) if user != current_user))
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

async fn describe_dbus_error(proxy: &GazeProxy<'_>, context: &str, err: &zbus::Error) -> String {
    if dbus_is_not_activatable(err) {
        let (problem, fix) = daemon_unavailable_hint(proxy.inner().connection()).await;
        format!("{context}: {problem}\n  {fix}")
    } else {
        format!("{context}: {}", dbus_error_message(err))
    }
}

fn exit_with_error(message: &str) -> ! {
    let _ = Term::stderr().write_line(&format!("{} {message}", style("✗").red().bold()));
    std::process::exit(1);
}

fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
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
    /// Test face authentication for a user
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
    /// Enroll a face with guided, multi-angle capture
    AddFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "The name of the face to enroll")]
        face: String,
    },
    /// Add captures to improve recognition of an enrolled face
    RefineFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "The name of the face to refine", add = ArgValueCompleter::new(face_completer))]
        face: String,
    },
    /// List the faces enrolled for a user
    ListFaces {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(long, help = "Print the faces as JSON")]
        json: bool,
    },
    /// Remove an enrolled face
    RemoveFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "The name of the face to remove", add = ArgValueCompleter::new(face_completer))]
        face: String,
    },
    /// Rename an enrolled face
    RenameFace {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(help = "Current face name", add = ArgValueCompleter::new(face_completer))]
        from: String,
        #[arg(help = "New face name")]
        to: String,
    },
    /// Remove all Gaze data for a user
    ClearUser {
        #[arg(short, long)]
        user: Option<String>,
        #[arg(short = 'y', long, help = "Skip the confirmation prompt")]
        yes: bool,
    },
    /// Show or clear the face authentication lockout set by a duress signal
    Duress {
        #[arg(long, help = "Re-enable face authentication after a duress lockout")]
        clear: bool,
        #[arg(short, long)]
        user: Option<String>,
    },
    /// Configure daemon and GDM settings interactively, or read and change one setting
    #[command(args_conflicts_with_subcommands = true)]
    Config {
        #[arg(long, help = "Print every setting as TOML and exit")]
        show: bool,
        #[command(subcommand)]
        action: Option<ConfigAction>,
    },
    /// Enroll or replace a TPM-protected GNOME Keyring or KWallet credential (root only)
    Keyring {
        /// Use KDE KWallet for the credential instead of GNOME Keyring
        #[arg(long)]
        kwallet: bool,
        /// Remove the stored credential instead of enrolling one
        #[arg(long)]
        forget: bool,
        #[arg(short, long, help = "Act on this user instead of the current one")]
        user: Option<String>,
    },
    /// Check the Gaze installation for configuration and runtime issues
    Doctor {
        #[arg(short, long, help = "Check enrollments for this user")]
        user: Option<String>,
        #[arg(
            short,
            long,
            help = "Benchmark detector, recognizer, and liveness model inference speed"
        )]
        benchmark: bool,
        #[arg(long, conflicts_with = "report", help = "Print the results as JSON")]
        json: bool,
        #[arg(long, help = "Write a redacted bug report file to attach to an issue")]
        report: bool,
    },
    /// Remove Gaze packages, PAM integration, configuration, models, and user data
    Uninstall {
        #[arg(short = 'y', long, help = "Skip the confirmation prompt")]
        yes: bool,
        #[arg(long, help = "Preserve /var/lib/gaze (enrolled face data)")]
        keep_data: bool,
        #[arg(long, help = "Print the planned commands without executing them")]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print one setting, such as `security.level`
    Get { key: String },
    /// Change one setting, such as `liveness.enabled true`
    Set { key: String, value: String },
}

async fn handle_enroll(
    proxy: &GazeProxy<'_>,
    user: &str,
    face: &str,
    is_refine: bool,
) -> anyhow::Result<()> {
    let term = Term::stdout();

    if let Err(err) = proxy.claim(user).await {
        exit_with_error(&describe_dbus_error(proxy, "Failed to claim device", &err).await);
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
        exit_with_error(&describe_dbus_error(proxy, "Failed to start enrollment", &err).await);
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
        Term::stderr().write_line(&format!(
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
        Term::stderr().write_line(&format!(
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
        Err(_) if silent => std::process::exit(1),
        Err(e) => {
            exit_with_error(&describe_dbus_error(proxy, "Failed to read enrolled faces", &e).await)
        }
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
        if silent {
            std::process::exit(1);
        }
        exit_with_error(&describe_dbus_error(proxy, "Failed to claim device", &err).await);
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
        let _ = proxy.release().await;
        if silent {
            std::process::exit(1);
        }
        exit_with_error(&describe_dbus_error(proxy, "Could not start the face scan", &e).await);
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
        // Collect these even without `--verbose`, so failures can include the latest diagnostic.
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

    // A diagnostic sent just before the verdict may still be in flight. Since the streams are
    // separate, this one can explain the result even if it arrives after the verdict.
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
            Term::stderr().write_line(&format!(
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
        let err_term = Term::stderr();
        err_term.write_line(&format!(
            "{} Authentication failed ({}ms)",
            style("✗").red().bold(),
            start.elapsed().as_millis()
        ))?;
        // "Authentication failed" alone can sound like a face mismatch even if the camera never
        // opened. Verbose mode has already printed the full diagnostic list.
        if !verbose && let Some(reason) = diagnostics.last() {
            err_term.write_line(&format!("  {}", style(reason).yellow()))?;
        }
    }

    let _ = proxy.release().await;
    if !authenticated {
        std::process::exit(1);
    }
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
        Commands::Keyring {
            forget,
            user,
            kwallet,
        } => {
            let backend = if *kwallet {
                gaze_security::keyring::Backend::KWallet
            } else {
                gaze_security::keyring::Backend::Gnome
            };
            let username = user.clone().unwrap_or_else(get_current_user);
            if *forget {
                gaze_security::keyring::forget_for(backend, &username)?;
                println!(
                    "Stored {} credential removed for {username}.",
                    backend.name()
                );
            } else {
                keyring::enroll(&username, &Config::load()?, backend)?;
            }
            return Ok(());
        }
        Commands::Uninstall {
            yes,
            keep_data,
            dry_run,
        } => return handle_uninstall(*yes, *keep_data, *dry_run),
        Commands::Doctor {
            user,
            benchmark,
            json,
            report,
        } => {
            let username = user.clone().unwrap_or_else(get_current_user);
            let output = if *json {
                doctor::Output::Json
            } else if *report {
                doctor::Output::Report
            } else {
                doctor::Output::Human
            };
            let healthy = doctor::run(&username, *benchmark, output).await?;
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
        Commands::ListFaces { user, json } => {
            handle_list_faces(&proxy, &user.unwrap_or_else(get_current_user), json).await?;
        }
        Commands::RemoveFace { user, face } => {
            handle_remove_face(&proxy, &user.unwrap_or_else(get_current_user), &face).await?;
        }
        Commands::RenameFace { user, from, to } => {
            handle_rename_face(&proxy, &user.unwrap_or_else(get_current_user), &from, &to).await?;
        }
        Commands::ClearUser { user, yes } => {
            handle_clear_user(&proxy, &user.unwrap_or_else(get_current_user), yes).await?;
        }
        Commands::Duress { clear, user } => {
            handle_duress(&proxy, &user.unwrap_or_else(get_current_user), clear).await?;
        }
        Commands::Config { show, action } => {
            let (config, keyring_supported) =
                match load_config_with_keyring_from_daemon(&proxy).await {
                    Ok(loaded) => loaded,
                    Err(err) => match err.downcast_ref::<zbus::Error>() {
                        Some(dbus) => exit_with_error(
                            &describe_dbus_error(&proxy, "Failed to read the configuration", dbus)
                                .await,
                        ),
                        None => return Err(err),
                    },
                };
            match action {
                Some(ConfigAction::Get { key }) => println!("{}", config.get_key(&key)?),
                Some(ConfigAction::Set { key, value }) => {
                    let updated = config.with_key(&key, &value)?;
                    let saved = if keyring_supported.gnome || keyring_supported.kwallet {
                        apply_config_with_keyring_to_daemon(&proxy, &updated).await
                    } else {
                        apply_config_to_daemon(&proxy, &updated).await
                    };
                    saved.map_err(|e| anyhow::anyhow!("Failed to save configuration: {e}"))?;
                    println!("{key} = {}", updated.get_key(&key)?);
                }
                None if show => print!("{}", config_show_text(&config)?),
                None => {
                    run_config_wizard(&Term::stdout(), &proxy, config, keyring_supported).await?
                }
            }
        }

        Commands::Doctor { .. } | Commands::Uninstall { .. } | Commands::Keyring { .. } => {
            unreachable!("handled before DBus connection")
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
                json: false,
                report: false,
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
    fn forgetting_a_keyring_credential_requires_root() {
        let cli = Cli::try_parse_from(["gaze", "keyring", "--forget"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Keyring { forget: true, .. }
        ));
        assert_eq!(command_requires_root(&cli.command), Some("keyring"));
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
    fn clearing_a_duress_lockout_always_needs_a_tty_polkit_agent() {
        let clear = Cli::try_parse_from(["gaze", "duress", "--clear"]).unwrap();
        assert!(command_may_be_challenged_as(&clear.command, false, "alice"));
        assert!(!command_may_be_challenged_as(&clear.command, true, "alice"));

        let own = Cli::try_parse_from(["gaze", "duress", "--clear", "--user", "alice"]).unwrap();
        assert!(command_may_be_challenged_as(&own.command, false, "alice"));

        let show = Cli::try_parse_from(["gaze", "duress"]).unwrap();
        assert!(!command_may_be_challenged_as(&show.command, false, "alice"));

        let other = Cli::try_parse_from(["gaze", "duress", "--user", "bob"]).unwrap();
        assert!(command_may_be_challenged_as(&other.command, false, "alice"));
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
    fn omarchy_plugin_version_matches_release() {
        let manifest = include_str!("../../../integrations/omarchy/manifest.json");
        let expected = format!("\"version\": \"{}\"", env!("CARGO_PKG_VERSION"));
        assert!(
            manifest.contains(&expected),
            "integrations/omarchy/manifest.json must carry version {}",
            env!("CARGO_PKG_VERSION")
        );
    }

    #[test]
    fn every_capture_status_maps_to_a_tone() {
        // `Ready` stays green through a brief dark dip; `Usable` is ready for matching.
        assert!(matches!(capture_tone(CaptureStatus::Ready), Tone::Good));
        assert!(matches!(capture_tone(CaptureStatus::Usable), Tone::Good));

        // Use red when the scan has nothing it can work with.
        assert!(matches!(capture_tone(CaptureStatus::Unused), Tone::Error));
        assert!(matches!(capture_tone(CaptureStatus::NoFace), Tone::Error));

        // Amber for something the user can correct by moving or turning a light on.
        for status in [
            CaptureStatus::TooDark,
            CaptureStatus::Clipped,
            CaptureStatus::NotCentered,
            CaptureStatus::TooFar,
            CaptureStatus::TooClose,
        ] {
            assert!(
                matches!(capture_tone(status), Tone::Warn),
                "{status:?} is a recoverable framing problem"
            );
        }
    }

    #[test]
    fn resolve_current_user_ignores_empty_environment_values() {
        assert_eq!(
            resolve_current_user(Some(String::new()), Some("alice".into())),
            "alice",
            "an empty SUDO_USER must not win over USER"
        );
        assert_eq!(
            resolve_current_user(Some(String::new()), Some(String::new())),
            "root"
        );
        assert_eq!(resolve_current_user(None, None), "root");
        assert_eq!(resolve_current_user(None, Some("bob".into())), "bob");
    }

    #[test]
    fn clear_user_and_config_get_set_parse() {
        let cli = Cli::try_parse_from(["gaze", "clear-user", "--yes"]).unwrap();
        assert!(matches!(cli.command, Commands::ClearUser { yes: true, .. }));
        let cli =
            Cli::try_parse_from(["gaze", "config", "set", "liveness.enabled", "true"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Config {
                action: Some(ConfigAction::Set { ref key, ref value }),
                ..
            } if key == "liveness.enabled" && value == "true"
        ));
        assert!(
            Cli::try_parse_from(["gaze", "config", "--show", "get", "security.level"]).is_err()
        );
        assert!(Cli::try_parse_from(["gaze", "doctor", "--json", "--report"]).is_err());
    }

    #[test]
    fn showing_the_config_stays_unprivileged_while_editing_it_does_not() {
        assert_eq!(
            command_requires_root(&Commands::Config {
                show: true,
                action: None
            }),
            None,
            "reading the config is not a privileged operation"
        );
        assert_eq!(
            command_requires_root(&Commands::Config {
                show: false,
                action: None
            }),
            Some("config")
        );
        assert_eq!(
            command_requires_root(&Commands::Config {
                show: false,
                action: Some(ConfigAction::Get {
                    key: "security.level".into()
                })
            }),
            None
        );
        assert_eq!(
            command_requires_root(&Commands::Config {
                show: false,
                action: Some(ConfigAction::Set {
                    key: "liveness.enabled".into(),
                    value: "true".into()
                })
            }),
            Some("config")
        );
    }

    #[test]
    fn only_the_commands_that_name_a_user_have_a_target() {
        assert_eq!(
            command_target_user(&Commands::Auth {
                user: Some("alice".into()),
                verbose: false,
                silent: false,
            }),
            Some("alice")
        );
        assert_eq!(
            command_target_user(&Commands::ListFaces {
                user: Some("bob".into()),
                json: false
            }),
            Some("bob")
        );
        assert_eq!(
            command_target_user(&Commands::Auth {
                user: None,
                verbose: false,
                silent: false,
            }),
            None
        );
        assert_eq!(
            command_target_user(&Commands::Config {
                show: true,
                action: None
            }),
            None,
            "config is never run against another account"
        );
    }

    #[test]
    fn the_escalation_marker_is_passed_through_to_the_re_executed_process() {
        assert_eq!(ESCALATION_MARKER, "GAZE_ESCALATED");
        assert!(
            ESCALATION_PRESERVED_ENV.contains(&"XDG_RUNTIME_DIR"),
            "the session bus address is derived from XDG_RUNTIME_DIR, so sudo must keep it"
        );
    }

    #[test]
    fn re_execution_refuses_to_loop_when_it_is_already_escalated() {
        if std::env::var_os(ESCALATION_MARKER).is_none() {
            return;
        }
        let err = reexec_as_root("add-face").expect_err("a second escalation would loop forever");
        assert!(err.to_string().contains("did not gain root privileges"));
    }
}
