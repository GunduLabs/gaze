use clap::{Parser, Subcommand};
use gaze_core::camera::Camera;
use gaze_core::capture::frame_to_bytes;
use gaze_core::config::Config;
use gaze_core::capture_session::{CaptureMode, CaptureState, CaptureSession};
use gaze_core::face::FaceChecker;
use std::io::{self, Write};
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

    if mode == CaptureMode::Guided {
        println!("\n  Face capture session for '{}/{}'\n", user, face);
    } else {
        println!("\n  Refining face '{}/{}'\n", user, face);
    }
    println!("  Position your face as prompted. Capture is automatic when centered.\n");

    let mut last_hint = String::new();
    let mut last_prompt = String::new();

    loop {
        if session.is_complete() {
            let captures = session.take_captures();
            for capture in captures {
                proxy
                    .add_face(user, face, &capture.bytes, capture.width, capture.height)
                    .await?;
            }
            println!(
                "\n  ✓ Mode complete! Captures saved for '{}/{}'.\n",
                user, face
            );
            break;
        }

        let frame = cam.capture_frame()?;
        let state = match session.process_frame(&frame) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  ✗ Error processing frame: {}", e);
                thread::sleep(Duration::from_millis(500));
                continue;
            }
        };

        match state {
            CaptureState::Prompting {
                prompt,
                step,
                total_steps,
                ..
            }
            | CaptureState::Countdown {
                prompt,
                step,
                total_steps,
                ..
            } => {
                let prompt_text = prompt.to_string();

                let hint = match &state {
                    CaptureState::Prompting { hint, .. } => hint.to_string(),
                    CaptureState::Countdown {
                        seconds_remaining, ..
                    } => {
                        format!("✓ Centered! Hold still for {:.1}s...", seconds_remaining)
                    }
                    _ => unreachable!(),
                };

                let msg = if step == 0 {
                    format!("  {}: {}", prompt_text, hint)
                } else {
                    format!("  [{}/{}] {}: {}", step, total_steps, prompt_text, hint)
                };

                last_prompt = prompt_text.to_string();
                if msg != last_hint {
                    eprint!("\r{}\x1b[K", msg);
                    io::stderr().flush().unwrap();
                    last_hint = msg;
                }
            }
            CaptureState::Captured { prompt: _ } => {
                eprint!("\r                                      \r");
                println!("  ✓ Captured {}!\n", last_prompt);
                thread::sleep(Duration::from_secs(1));
                last_hint.clear();
            }
            CaptureState::Complete => break,
        }
        thread::sleep(Duration::from_millis(30));
    }

    Ok(())
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
        user: String,
        #[arg(long, help = "Print detailed step-by-step performance metrics")]
        perf: bool,
        #[arg(short, long, help = "Show detailed authentication metrics")]
        verbose: bool,
    },
    /// Capture a new face with guided multi-angle session
    AddFace {
        #[arg(short, long)]
        user: String,
        #[arg(short, long)]
        face: String,
    },
    /// Add additional captures to improve recognition of an existing face
    RefineFace {
        #[arg(short, long)]
        user: String,
        #[arg(short, long)]
        face: String,
    },
    /// Remove a named face for a user
    RemoveFace {
        #[arg(short, long)]
        user: String,
        #[arg(short, long)]
        face: String,
    },
    /// Remove all data for a user
    ClearUser {
        #[arg(short, long)]
        user: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;
    let conn = Connection::system().await?;
    let proxy = AuthProxy::new(&conn).await?;

    match cli.command {
        Commands::Auth {
            user,
            perf,
            verbose,
        } => {
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
                    "[gaze perf] {:>40} | step: {:>8.3}ms | total: {:>8.3}ms",
                    label,
                    delta.as_secs_f64() * 1000.0,
                    total.as_secs_f64() * 1000.0,
                );
            };

            let mut cam = Camera::open(&config.cameras.rgb)?;
            log("camera opened", &mut last);
            let mut authenticated = false;
            let mut denied = false;

            for attempt in 0..10 {
                let frame = cam.capture_frame()?;
                log(&format!("attempt {attempt}: frame captured"), &mut last);
                let result = frame_to_bytes(&frame)?;
                log(&format!("attempt {attempt}: frame_to_bytes"), &mut last);
                match proxy
                    .match_faces(&user, &result.bytes, result.width, result.height)
                    .await
                {
                    Ok(faces) => {
                        log(&format!("attempt {attempt}: match complete"), &mut last);
                        let matched = faces.iter().find(|(_, _, _, passed, _)| *passed);

                        if verbose {
                            println!();
                            println!(
                                "{:<20} {:>10} {:>8} {:>8} {:>6}",
                                "Face", "Similarity", "Match %", "Passed", "Count"
                            );
                            println!("{}", "-".repeat(56));
                            for (name, score, pct, passed, count) in &faces {
                                println!(
                                    "{:<20} {:>10.4} {:>7.1}% {:>8} {:>6}",
                                    name,
                                    score,
                                    pct,
                                    if *passed { "✓" } else { "✗" },
                                    count,
                                );
                            }
                            println!();
                        }

                        if let Some((face, _, pct, _, _)) = matched {
                            println!(
                                "\x1b[32mAuthenticated as: {} ({:.1}%, {}ms)\x1b[0m",
                                face,
                                pct,
                                start.elapsed().as_millis()
                            );
                            authenticated = true;
                        } else {
                            println!(
                                "\x1b[31mAccess Denied. ({}ms)\x1b[0m",
                                start.elapsed().as_millis()
                            );
                            denied = true;
                        }
                        break;
                    }
                    Err(ref err) if err.to_string().contains("RETRYABLE:") => {
                        log(&format!("attempt {attempt}: retryable error"), &mut last);
                        if verbose {
                            println!("  attempt {}: {}", attempt, err);
                        }
                        continue;
                    }
                    Err(err) => return Err(err.into()),
                }
            }

            if !authenticated && !denied {
                println!("\x1b[33mAccess Denied. Could not detect a face.\x1b[0m");
            }
        }
        Commands::AddFace { ref user, ref face } | Commands::RefineFace { ref user, ref face } => {
            let mode = if matches!(cli.command, Commands::AddFace { .. }) {
                CaptureMode::Guided
            } else {
                CaptureMode::Refine
            };
            let mut cam = Camera::open(&config.cameras.rgb)?;
            let checker = FaceChecker::new()?;
            run_capture_session(&proxy, &mut cam, checker, user, face, mode).await?;
        }
        Commands::RemoveFace { user, face } => {
            let removed = proxy.remove_face(&user, &face).await?;
            if removed {
                println!("Face '{}' removed for '{}'", face, user);
            } else {
                println!("Face '{}' not found for '{}'", face, user);
            }
        }
        Commands::ClearUser { user } => {
            let cleared = proxy.clear_user(&user).await?;
            if cleared {
                println!("All data cleared for '{}'", user);
            } else {
                println!("No data found for '{}'", user);
            }
        }
    }

    Ok(())
}
