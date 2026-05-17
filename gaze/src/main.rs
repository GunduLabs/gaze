mod align;
mod daemon;
pub mod models;
mod recognize;
pub mod users;

use crate::users::UserDatabase;
use daemon::AuthDaemon;
use gaze_core::config::{Config, MODELS_DIR, USERS_DIR};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;
use tracing_subscriber::EnvFilter;
use zbus::connection::Builder;

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

    let db = UserDatabase::new(USERS_DIR, config.enrollment.max_templates as usize)?;

    let checker = gaze_core::face::FaceChecker::from_detector_with_config(detector, &config);

    let daemon = AuthDaemon {
        checker: Arc::new(Mutex::new(checker)),
        recognizer: Arc::new(Mutex::new(recognizer)),
        db: Arc::new(Mutex::new(db)),
        threshold: Arc::new(Mutex::new(security.threshold())),
        camera_config: Arc::new(Mutex::new(config.cameras.rgb.clone())),
        claim_state: Arc::new(Mutex::new(None)),
        active_cancel: Arc::new(Mutex::new(None)),
        rt_handle: tokio::runtime::Handle::current(),
    };

    info!(elapsed = ?t_load.elapsed(), "Models & user DB loaded");

    if let Ok(uid) = daemon::get_active_session_uid().await {
        daemon::set_pipewire_runtime_for_uid(uid);
    }

    let _conn = Builder::system()?
        .name("com.gundulabs.Gaze")?
        .serve_at("/com/gundulabs/Gaze", daemon)?
        .build()
        .await?;

    info!("Gaze Daemon listening on System Bus");
    std::future::pending::<()>().await;

    Ok(())
}
