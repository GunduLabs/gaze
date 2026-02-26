use clap::{Parser, Subcommand};
use gaze_core::camera::Camera;
use gaze_core::capture::{CaptureStatus, frame_to_bytes, wait_for_centered_capture};
use gaze_core::config::Config;
use gaze_core::face::FaceChecker;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;
use zbus::Connection;

use gaze_core::dbus::AuthProxy;

fn print_status(status: &CaptureStatus) {
    match status {
        CaptureStatus::NoFace => eprint!("\r  ⏳ No face detected...          "),
        CaptureStatus::NotCentered => eprint!("\r  ⏳ Center your face...           "),
        _ => {}
    }
    io::stderr().flush().unwrap();
}

const ENROLLMENT_PROMPTS: &[&str] = &[
    "Look straight at the camera",
    "Turn your head slightly to the LEFT",
    "Turn your head slightly to the RIGHT",
    "Tilt your head slightly UP",
];

async fn guided_enrollment(
    proxy: &AuthProxy<'_>,
    cam: &mut Camera,
    checker: &mut FaceChecker,
    user: &str,
    face: &str,
) -> anyhow::Result<()> {
    println!("\n  Face enrollment for '{}/{}'\n", user, face);
    println!("  Position your face as prompted. Capture is automatic when centered.\n");

    for (idx, prompt) in ENROLLMENT_PROMPTS.iter().enumerate() {
        println!("  [{}/{}] {}", idx + 1, ENROLLMENT_PROMPTS.len(), prompt);

        loop {
            let result = wait_for_centered_capture(cam, checker, print_status)?;
            eprint!("\r                                      \r");

            match proxy
                .add_face(user, face, &result.bytes, result.width, result.height)
                .await
            {
                Ok(_) => {
                    println!("  ✓ Captured!\n");
                    thread::sleep(Duration::from_secs(1));
                    break;
                }
                Err(err) => {
                    eprintln!("  ✗ {}, retrying...", err);
                    thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }

    println!(
        "  ✓ Enrollment complete! {} angles captured for '{}/{}'.\n",
        ENROLLMENT_PROMPTS.len(),
        user,
        face
    );
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
    },
    /// Enroll a new face with guided multi-angle capture
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
        Commands::Auth { user, perf } => {
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
                eprintln!(
                    "[gaze perf] {:>40} | step: {:>8.3}ms | total: {:>8.3}ms",
                    label,
                    delta.as_secs_f64() * 1000.0,
                    total.as_secs_f64() * 1000.0,
                );
            };

            let mut cam = Camera::open(&config.cameras.rgb)?;
            log("camera opened", &mut last);
            let mut authenticated = false;

            for attempt in 0..10 {
                let frame = cam.capture_frame()?;
                log(&format!("attempt {attempt}: frame captured"), &mut last);
                let result = frame_to_bytes(&frame)?;
                log(&format!("attempt {attempt}: frame_to_bytes"), &mut last);
                match proxy
                    .authenticate(&user, &result.bytes, result.width, result.height)
                    .await
                {
                    Ok(face) if !face.is_empty() => {
                        log(&format!("attempt {attempt}: auth SUCCESS"), &mut last);
                        println!(
                            "Authenticated as: {} ({}ms)",
                            face,
                            start.elapsed().as_millis()
                        );
                        authenticated = true;
                        break;
                    }
                    Ok(_) => {
                        log(&format!("attempt {attempt}: no face matched"), &mut last);
                        println!("Access Denied. ({}ms)", start.elapsed().as_millis());
                        break;
                    }
                    Err(ref err) if err.to_string().contains("RETRYABLE:") => {
                        log(&format!("attempt {attempt}: retryable error"), &mut last);
                        continue;
                    }
                    Err(err) => return Err(err.into()),
                }
            }

            if !authenticated {
                println!("Access Denied.");
            }
        }
        Commands::AddFace { user, face } => {
            let mut cam = Camera::open(&config.cameras.rgb)?;
            let mut checker = FaceChecker::new()?;
            guided_enrollment(&proxy, &mut cam, &mut checker, &user, &face).await?;
        }
        Commands::RefineFace { user, face } => {
            let mut cam = Camera::open(&config.cameras.rgb)?;
            let mut checker = FaceChecker::new()?;

            println!(
                "\n  Refining face '{}/{}'. Auto-capturing when face is centered.",
                user, face
            );
            println!("  Older captures are replaced when the limit is reached.");
            println!("  Press Ctrl+C to stop.\n");

            let mut count = 0;
            loop {
                println!("  Waiting for centered face...");
                let result = wait_for_centered_capture(&mut cam, &mut checker, print_status)?;
                eprint!("\r                                      \r");

                match proxy
                    .add_face(&user, &face, &result.bytes, result.width, result.height)
                    .await
                {
                    Ok(_) => {
                        count += 1;
                        println!("  ✓ Capture #{} added!\n", count);
                    }
                    Err(err) => eprintln!("  ✗ {}\n", err),
                }

                thread::sleep(Duration::from_secs(2));
            }
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
