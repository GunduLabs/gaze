#![allow(dead_code, unused_imports)]

#[path = "daemon/align.rs"]
mod align;
mod daemon;
#[path = "daemon/models.rs"]
pub mod models;
#[path = "daemon/recognize.rs"]
mod recognize;
#[path = "daemon/users.rs"]
pub mod users;

use daemon::AuthDaemon;
use gaze_common::config::Config;
use std::sync::Arc;
use tokio::sync::Mutex;
use zbus::ConnectionBuilder;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Initializing Gaze Daemon...");
    let t_load = std::time::Instant::now();

    let config = Config::load()?;
    let security = &config.security;

    println!(
        "Security: {:?} | Detector: {} | Recognizer: {} | Threshold: {}",
        security,
        security.detector(),
        security.recognizer(),
        security.threshold()
    );

    let (det_path, rec_path) = models::ensure_models(
        &config.storage.models_dir,
        security.detector(),
        security.recognizer(),
    )?;

    let detector = gaze_common::detect::FaceDetector::new(det_path.to_str().unwrap())
        .expect("Failed to load detection model");
    let checker = gaze_common::face::FaceChecker::from_detector(detector);

    let recognizer = recognize::FaceRecognizer::new(rec_path.to_str().unwrap())
        .expect("Failed to load recognition model");

    let db = users::UserDatabase::new(&config.storage.users_dir)?;

    let daemon = AuthDaemon {
        checker: Arc::new(Mutex::new(checker)),
        recognizer: Arc::new(Mutex::new(recognizer)),
        db: Arc::new(Mutex::new(db)),
        threshold: security.threshold(),
        max_captures: config.enrollment.max_captures_per_face,
    };

    println!("Models & User DB loaded in: {:?}", t_load.elapsed());

    let _conn = ConnectionBuilder::system()?
        .name("org.gaze.Auth")?
        .serve_at("/org/gaze/Auth", daemon)?
        .build()
        .await?;

    println!("Gaze Daemon listening on System Bus...");
    std::future::pending::<()>().await;

    Ok(())
}
