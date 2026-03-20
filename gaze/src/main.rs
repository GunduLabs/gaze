#![allow(dead_code, unused_imports)]

mod align;
mod daemon;
pub mod models;
mod recognize;
pub mod users;

use daemon::AuthDaemon;
use gaze_core::config::{Config, MODELS_DIR, USERS_DIR};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;
use zbus::ConnectionBuilder;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Initializing Gaze Daemon...");
    let t_load = std::time::Instant::now();

    let config = Config::load()?;
    let security = &config.security;

    info!(
        level = ?security,
        detector = security.detector(),
        recognizer = security.recognizer(),
        threshold = security.threshold(),
        "Loaded security config"
    );

    let (det_path, rec_path) =
        models::ensure_models(MODELS_DIR, security.detector(), security.recognizer())?;

    let detector = gaze_core::detect::FaceDetector::new(det_path.to_str().unwrap())
        .expect("Failed to load detection model");

    let recognizer = recognize::FaceRecognizer::new(rec_path.to_str().unwrap())
        .expect("Failed to load recognition model");

    let db = users::UserDatabase::new(USERS_DIR)?;

    let daemon = AuthDaemon {
        detector: Arc::new(Mutex::new(detector)),
        recognizer: Arc::new(Mutex::new(recognizer)),
        db: Arc::new(Mutex::new(db)),
        threshold: security.threshold(),
        max_captures: config.enrollment.max_captures_per_face,
    };

    info!(elapsed = ?t_load.elapsed(), "Models & user DB loaded");

    let _conn = ConnectionBuilder::system()?
        .name("org.gaze.Auth")?
        .serve_at("/org/gaze/Auth", daemon)?
        .build()
        .await?;

    info!("Gaze Daemon listening on System Bus");
    std::future::pending::<()>().await;

    Ok(())
}
