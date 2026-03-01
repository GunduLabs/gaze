use clap::{Parser, Subcommand};
use console::{Term, style};
use gaze_core::camera::Camera;
use gaze_core::capture::{init_camera_and_checker, wait_for_capture};
use gaze_core::capture_session::{CaptureMode, CaptureSession, CaptureState};
use gaze_core::config::Config;
use gaze_core::face::{CaptureStatus, FaceChecker};
use indicatif::{ProgressBar, ProgressStyle};
use std::thread;
use std::time::Duration;
use zbus::Connection;

use gaze_core::dbus::AuthProxy;

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
                        pb.set_prefix(prefix);
                        pb.set_message(format!(
                            "{}: {}",
                            style(prompt).white().bold(),
                            style(hint).yellow()
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
                            style("Hold still!").green().bold()
                        ));
                    }
                    CaptureState::Captured { prompt } => {
                        pb.set_style(style_captured.clone());
                        pb.set_message(format!(
                            "{}",
                            style(format!("Captured {}!", prompt)).green().bold()
                        ));
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
    /// Remove all data for a user
    ClearUser {
        #[arg(short, long)]
        user: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
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
                    CaptureStatus::NoFace => {
                        format!(
                            "{}",
                            style("No face detected. Please look at the camera...").red()
                        )
                    }
                    CaptureStatus::Clipped(_) => {
                        format!(
                            "{}",
                            style("Face is clipped. Move fully into frame...").yellow()
                        )
                    }
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
                    return Err(err.into());
                }
            }
        }
        Commands::AddFace { user, face } => {
            let user = user.unwrap_or_else(get_current_user);
            let mut cam = Camera::open(&config.cameras.rgb)?;
            let checker = FaceChecker::new()?;
            run_capture_session(&proxy, &mut cam, checker, &user, &face, CaptureMode::Guided)
                .await?;
        }
        Commands::RefineFace { user, face } => {
            let user = user.unwrap_or_else(get_current_user);
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
                    term.write_line(&format!(
                        "{} Failed to fetch faces: {}",
                        style("✗").red().bold(),
                        e
                    ))?;
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

            let removed = proxy.remove_face(&user, &face).await?;
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

            let cleared = proxy.clear_user(&user).await?;
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
    }

    Ok(())
}
