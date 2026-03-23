use clap::{Parser, Subcommand};
use console::{Term, style};
use dialoguer::{Input, Select, theme::ColorfulTheme};
use gaze_core::camera::Camera;
use gaze_core::capture::{init_camera_and_checker, wait_for_capture};
use gaze_core::capture_session::{CaptureHint, CaptureMode, CaptureSession, CaptureState};
use gaze_core::config::{Config, SecurityLevel};
use gaze_core::dbus::{
    AuthProxy, apply_config_to_daemon, dbus_error_message, dbus_is_file_not_found,
    load_config_from_daemon,
};
use gaze_core::face::{CaptureStatus, FaceChecker};
use indicatif::{ProgressBar, ProgressStyle};
use std::thread;
use std::time::Duration;
use zbus::Connection;

async fn run_capture_session(
    proxy: &AuthProxy<'_>,
    cam: &mut Camera,
    checker: FaceChecker,
    user: &str,
    face: &str,
    mode: CaptureMode,
) -> anyhow::Result<()> {
    let mut session = CaptureSession::new(checker).with_mode(mode);
    session.start();

    let term = Term::stdout();
    term.clear_screen()?;

    if mode == CaptureMode::Guided {
        term.write_line(&format!(
            "\n  {} {}\n",
            style("Face capture session for").cyan().bold(),
            style(format!("{}/{}", user, face)).cyan().underlined()
        ))?;
    } else {
        term.write_line(&format!(
            "\n  {} {}\n",
            style("Refining face").cyan().bold(),
            style(format!("{}/{}", user, face)).cyan().underlined()
        ))?;
    }
    term.write_line("  Position your face as prompted. Capture is automatic when centered.\n")?;

    let pb = ProgressBar::new(100);
    let style_prompting = ProgressStyle::default_spinner()
        .template("{spinner:.yellow} {prefix:.bold.blue} {msg}")
        .unwrap()
        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");

    let style_countdown = ProgressStyle::default_bar()
        .template("{spinner:.green} {prefix:.bold.blue} {msg} {bar:20.green/blue} {percent}%")
        .unwrap()
        .progress_chars("█▇▆▅▄▃▂   ");

    let style_captured = ProgressStyle::default_spinner()
        .template("{spinner:.green} {prefix:.bold.green} {msg}")
        .unwrap()
        .tick_chars("✓   ");

    pb.set_style(style_prompting.clone());
    pb.enable_steady_tick(Duration::from_millis(80));

    loop {
        if session.is_complete() {
            let captures = session.take_captures();
            pb.set_style(style_captured.clone());
            pb.finish_with_message(format!("Saving {} captures...", captures.len()));

            for capture in captures {
                proxy
                    .add_face(user, face, &capture.bytes, capture.width, capture.height)
                    .await?;
            }
            term.write_line(&format!(
                "\n  {} Captures perfectly saved for {}/{}!\n",
                style("✓").green().bold(),
                style(user).green(),
                style(face).green()
            ))?;
            break;
        }

        let frame = match cam.capture_frame() {
            Ok(f) => f,
            Err(e) => {
                pb.set_message(format!("{}", style(format!("Camera error: {}", e)).red()));
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        };

        match session.process_frame(&frame) {
            Ok(state) => {
                match state {
                    CaptureState::Prompting {
                        prompt,
                        step,
                        total_steps,
                        hint,
                        ..
                    } => {
                        pb.set_style(style_prompting.clone());
                        let prefix = if step == 0 {
                            "Step 1".to_string()
                        } else {
                            format!("Step {}/{}", step, total_steps)
                        };
                        let hint_text = style(format!("{}...", hint));
                        pb.set_prefix(prefix);
                        pb.set_message(format!(
                            "{}: {}",
                            style(prompt).white().bold(),
                            match hint {
                                CaptureHint::NoFace => hint_text.red(),
                                CaptureHint::NotCentered | CaptureHint::FaceClipped =>
                                    hint_text.yellow(),
                                CaptureHint::Ready => hint_text.green(),
                            }
                        ));
                    }
                    CaptureState::Countdown {
                        prompt,
                        step,
                        total_steps,
                        seconds_remaining,
                        ..
                    } => {
                        pb.set_style(style_countdown.clone());
                        let prefix = if step == 0 {
                            "Step 1".to_string()
                        } else {
                            format!("Step {}/{}", step, total_steps)
                        };
                        pb.set_prefix(prefix);

                        let progress =
                            ((1.5 - seconds_remaining) / 1.5 * 100.0).clamp(0.0, 100.0) as u64;
                        pb.set_position(progress);
                        pb.set_message(format!(
                            "{}: {}",
                            style(prompt).white().bold(),
                            style(format!("{}...", CaptureHint::Ready)).green().bold()
                        ));
                    }
                    CaptureState::Captured { prompt: _ } => {
                        pb.set_style(style_captured.clone());
                        pb.set_message(format!("{}", style("Captured.").green().bold()));
                        thread::sleep(Duration::from_millis(800));
                    }
                    CaptureState::Complete => {
                        // Handled above in is_complete
                    }
                }
            }
            Err(e) => {
                pb.set_message(format!(
                    "{}",
                    style(format!("Processing error: {}", e)).red()
                ));
            }
        }
        thread::sleep(Duration::from_millis(30));
    }

    Ok(())
}

fn get_current_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".into())
}

#[derive(Parser)]
#[command(name = "gaze", about = "Gaze Facial Authentication CLI")]
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
        #[arg(long, help = "Print detailed step-by-step performance metrics")]
        perf: bool,
        #[arg(short, long, help = "Show detailed authentication metrics")]
        verbose: bool,
    },
    /// Capture a new face with guided multi-angle session
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
}

async fn run_config_wizard(
    term: &Term,
    proxy: &AuthProxy<'_>,
    mut config: Config,
) -> anyhow::Result<()> {
    let theme = ColorfulTheme::default();

    term.write_line(&format!(
        "\n{}\n",
        style("Gaze Config Wizard").cyan().bold()
    ))?;

    let level_options = ["low", "medium", "high", "maximum", "custom"];
    let default_level_idx = match config.security {
        SecurityLevel::Low => 0,
        SecurityLevel::Medium => 1,
        SecurityLevel::High => 2,
        SecurityLevel::Maximum => 3,
        SecurityLevel::Custom { .. } => 4,
    };

    let selected = Select::with_theme(&theme)
        .with_prompt("Security level")
        .items(level_options)
        .default(default_level_idx)
        .interact()?;

    config.security = match selected {
        0 => SecurityLevel::Low,
        1 => SecurityLevel::Medium,
        2 => SecurityLevel::High,
        3 => SecurityLevel::Maximum,
        _ => {
            let (default_detector, default_recognizer, default_threshold) = match &config.security {
                SecurityLevel::Custom {
                    detector,
                    recognizer,
                    threshold,
                } => (detector.clone(), recognizer.clone(), *threshold),
                _ => (
                    config.security.detector().to_string(),
                    config.security.recognizer().to_string(),
                    config.security.threshold(),
                ),
            };

            let detector = Input::with_theme(&theme)
                .with_prompt("Custom detector model")
                .default(default_detector)
                .interact_text()?;

            let recognizer = Input::with_theme(&theme)
                .with_prompt("Custom recognizer model")
                .default(default_recognizer)
                .interact_text()?;

            let threshold = Input::with_theme(&theme)
                .with_prompt("Custom threshold (0.0 - 1.0)")
                .default(default_threshold)
                .interact_text()?;

            SecurityLevel::Custom {
                detector,
                recognizer,
                threshold,
            }
        }
    };

    config.cameras.rgb = Input::with_theme(&theme)
        .with_prompt("RGB camera device")
        .default(config.cameras.rgb.clone())
        .interact_text()?;

    config.enrollment.max_captures_per_face = Input::with_theme(&theme)
        .with_prompt("Max captures per face")
        .default(config.enrollment.max_captures_per_face)
        .interact_text()?;

    apply_config_to_daemon(proxy, &config).await?;
    term.write_line(&format!(
        "{} Configuration saved. Daemon will restart to apply changes.",
        style("✓").green().bold()
    ))?;

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let conn = Connection::system().await?;
    let proxy = AuthProxy::new(&conn).await?;

    let term = Term::stdout();

    match cli.command {
        Commands::Auth {
            user,
            perf,
            verbose,
        } => {
            let user = user.unwrap_or_else(get_current_user);
            let config = load_config_from_daemon(&proxy).await?;
            let mut last = std::time::Instant::now();
            let start = last;
            let log = |label: &str, last: &mut std::time::Instant| {
                if !perf {
                    return;
                }
                let now = std::time::Instant::now();
                let delta = now.duration_since(*last);
                let total = now.duration_since(start);
                *last = now;
                println!(
                    "{} {:>40} | step: {:>8.3}ms | total: {:>8.3}ms",
                    style("[gaze perf]").cyan(),
                    label,
                    delta.as_secs_f64() * 1000.0,
                    total.as_secs_f64() * 1000.0,
                );
            };

            let pb = ProgressBar::new_spinner();
            let pb_style = ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {prefix:.bold.blue} {msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
            pb.set_style(pb_style);
            pb.enable_steady_tick(Duration::from_millis(80));
            pb.set_prefix("Authenticating");
            pb.set_message("Opening camera...");

            let (mut cam, mut checker) = init_camera_and_checker(&config.cameras.rgb)?;
            log("camera + checker initialized", &mut last);

            pb.set_message(format!(
                "Scanning face for {}...",
                style(&user).cyan().bold()
            ));
            let result = wait_for_capture(&mut cam, &mut checker, false, |status| {
                let hint = match status {
                    CaptureStatus::NoFaces => style(status.to_string()).red().to_string(),
                    CaptureStatus::Clipped(_) => style(status.to_string()).yellow().to_string(),
                    _ => panic!("Unexpected capture status during authentication"),
                };

                if !hint.is_empty() {
                    pb.set_message(hint);
                }
            })?;
            log("ready capture", &mut last);

            match proxy
                .match_faces(&user, &result.bytes, result.width, result.height)
                .await
            {
                Ok(faces) => {
                    log("match complete", &mut last);
                    let matched = faces.iter().find(|(_, _, _, passed, _)| *passed);

                    if verbose {
                        pb.suspend(|| {
                            println!();
                            println!(
                                "{}",
                                style(format!(
                                    "{:<20} {:>10} {:>8} {:>8} {:>6}",
                                    "Face", "Similarity", "Match %", "Passed", "Count"
                                ))
                                .bold()
                            );
                            println!("{}", style("-".repeat(56)).dim());
                            for (name, score, pct, passed, count) in &faces {
                                let check = if *passed {
                                    style("✓").green()
                                } else {
                                    style("✗").red()
                                };
                                println!(
                                    "{:<20} {:>10.4} {:>7.1}% {:>8} {:>6}",
                                    style(name).cyan(),
                                    score,
                                    pct,
                                    check,
                                    count,
                                );
                            }
                            println!();
                        });
                    }

                    pb.finish_and_clear();
                    if let Some((face, _, pct, _, _)) = matched {
                        term.write_line(&format!(
                            "{} Authenticated as: {} ({:.1}%, {}ms)",
                            style("✓").green().bold(),
                            style(face).green().bold(),
                            pct,
                            start.elapsed().as_millis()
                        ))?;
                    } else {
                        term.write_line(&format!(
                            "{} Access Denied. ({}ms)",
                            style("✗").red().bold(),
                            start.elapsed().as_millis()
                        ))?;
                    }
                }
                Err(err) => {
                    pb.finish_and_clear();
                    term.write_line(&format!(
                        "{} Authentication error: {}",
                        style("✗").red().bold(),
                        dbus_error_message(&err)
                    ))?;
                }
            }
        }
        Commands::AddFace { user, face } => {
            let user = user.unwrap_or_else(get_current_user);
            let config = load_config_from_daemon(&proxy).await?;
            let mut cam = Camera::open(&config.cameras.rgb)?;
            let checker = FaceChecker::new()?;
            run_capture_session(&proxy, &mut cam, checker, &user, &face, CaptureMode::Guided)
                .await?;
        }
        Commands::RefineFace { user, face } => {
            let user = user.unwrap_or_else(get_current_user);
            let config = load_config_from_daemon(&proxy).await?;
            let mut cam = Camera::open(&config.cameras.rgb)?;
            let checker = FaceChecker::new()?;
            run_capture_session(&proxy, &mut cam, checker, &user, &face, CaptureMode::Refine)
                .await?;
        }
        Commands::ListFaces { user } => {
            let user = user.unwrap_or_else(get_current_user);
            let pb = ProgressBar::new_spinner();
            let pb_style = ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {prefix:.bold.blue} {msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
            pb.set_style(pb_style);
            pb.enable_steady_tick(Duration::from_millis(80));
            pb.set_prefix("Database");
            pb.set_message(format!(
                "Fetching faces for {}...",
                style(&user).cyan().bold()
            ));

            match proxy.list_faces(&user).await {
                Ok(faces) => {
                    pb.finish_and_clear();
                    if faces.is_empty() {
                        term.write_line(&format!(
                            "{} No faces found for {}",
                            style("i").cyan().bold(),
                            style(&user).bold()
                        ))?;
                    } else {
                        term.write_line(&format!(
                            "\n{} faces for {}:\n",
                            style(faces.len()).green().bold(),
                            style(&user).bold()
                        ))?;
                        for (face, count) in faces {
                            term.write_line(&format!(
                                "  {} {} ({} captures)",
                                style("•").cyan(),
                                style(face).bold(),
                                count
                            ))?;
                        }
                        term.write_line("")?;
                    }
                }
                Err(e) => {
                    pb.finish_and_clear();
                    if dbus_is_file_not_found(&e) {
                        term.write_line(&format!(
                            "{} No faces found for {}",
                            style("i").cyan().bold(),
                            style(&user).bold()
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
        }
        Commands::RemoveFace { user, face } => {
            let user = user.unwrap_or_else(get_current_user);
            let pb = ProgressBar::new_spinner();
            let pb_style = ProgressStyle::default_spinner()
                .template("{spinner:.red} {prefix:.bold.red} {msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
            pb.set_style(pb_style);
            pb.enable_steady_tick(Duration::from_millis(80));
            pb.set_prefix("Removing");
            pb.set_message(format!("Deleting face {}...", style(&face).red().bold()));

            let removed = match proxy.remove_face(&user, &face).await {
                Ok(value) => value,
                Err(err) => {
                    pb.finish_and_clear();
                    term.write_line(&format!(
                        "{} Failed to remove face: {}",
                        style("✗").red().bold(),
                        dbus_error_message(&err)
                    ))?;
                    return Ok(());
                }
            };
            pb.finish_and_clear();
            if removed {
                term.write_line(&format!(
                    "{} Face '{}' removed for '{}'",
                    style("✓").green().bold(),
                    face,
                    user
                ))?;
            } else {
                term.write_line(&format!(
                    "{} Face '{}' not found for '{}'",
                    style("!").yellow().bold(),
                    face,
                    user
                ))?;
            }
        }
        Commands::RenameFace { user, from, to } => {
            let user = user.unwrap_or_else(get_current_user);
            let pb = ProgressBar::new_spinner();
            let pb_style = ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {prefix:.bold.blue} {msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
            pb.set_style(pb_style);
            pb.enable_steady_tick(Duration::from_millis(80));
            pb.set_prefix("Renaming");
            pb.set_message(format!(
                "Renaming face {} -> {}...",
                style(&from).cyan().bold(),
                style(&to).cyan().bold()
            ));

            let renamed = match proxy.rename_face(&user, &from, &to).await {
                Ok(value) => value,
                Err(err) => {
                    pb.finish_and_clear();
                    term.write_line(&format!(
                        "{} Failed to rename face: {}",
                        style("✗").red().bold(),
                        dbus_error_message(&err)
                    ))?;
                    return Ok(());
                }
            };
            pb.finish_and_clear();
            if renamed {
                term.write_line(&format!(
                    "{} Face '{}' renamed to '{}' for '{}'",
                    style("✓").green().bold(),
                    from,
                    to,
                    user
                ))?;
            } else {
                term.write_line(&format!(
                    "{} Face '{}' not found for '{}'",
                    style("!").yellow().bold(),
                    from,
                    user
                ))?;
            }
        }
        Commands::ClearUser { user } => {
            let user = user.unwrap_or_else(get_current_user);
            let pb = ProgressBar::new_spinner();
            let pb_style = ProgressStyle::default_spinner()
                .template("{spinner:.red} {prefix:.bold.red} {msg}")
                .unwrap()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ");
            pb.set_style(pb_style);
            pb.enable_steady_tick(Duration::from_millis(80));
            pb.set_prefix("Clearing");
            pb.set_message(format!(
                "Deleting all data for {}...",
                style(&user).red().bold()
            ));

            let cleared = match proxy.clear_user(&user).await {
                Ok(value) => value,
                Err(err) => {
                    pb.finish_and_clear();
                    term.write_line(&format!(
                        "{} Failed to clear user: {}",
                        style("✗").red().bold(),
                        dbus_error_message(&err)
                    ))?;
                    return Ok(());
                }
            };
            pb.finish_and_clear();
            if cleared {
                term.write_line(&format!(
                    "{} All data cleared for '{}'",
                    style("✓").green().bold(),
                    user
                ))?;
            } else {
                term.write_line(&format!(
                    "{} No data found for '{}'",
                    style("!").yellow().bold(),
                    user
                ))?;
            }
        }
        Commands::Config { show } => {
            let config = load_config_from_daemon(&proxy).await?;

            if show {
                term.write_line(&format!(
                    "{} {}",
                    style("security.level:").bold(),
                    config.security.as_name()
                ))?;
                term.write_line(&format!(
                    "{} {}",
                    style("cameras.rgb:").bold(),
                    config.cameras.rgb
                ))?;
                term.write_line(&format!(
                    "{} {}",
                    style("enrollment.max_captures_per_face:").bold(),
                    config.enrollment.max_captures_per_face
                ))?;
                return Ok(());
            }

            run_config_wizard(&term, &proxy, config).await?;
        }
    }

    Ok(())
}
