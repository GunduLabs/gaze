#![allow(dead_code, unused_imports)]

mod daemon;

use daemon::AuthDaemon;
use gaze_core::config::Config;
use gaze_core::models;
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

    let detector = gaze_core::detect::FaceDetector::new(det_path.to_str().unwrap())
        .expect("Failed to load detection model");

    let recognizer = gaze_core::recognize::FaceRecognizer::new(rec_path.to_str().unwrap())
        .expect("Failed to load recognition model");

    let db = gaze_core::users::UserDatabase::new(&config.storage.users_dir)?;

    let daemon = AuthDaemon {
        detector: Arc::new(Mutex::new(detector)),
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
