mod align;
mod daemon;
mod liveness;
pub mod models;
mod recognize;
pub mod users;

use crate::users::UserDatabase;
use daemon::AuthDaemon;
use gaze_core::config::{Config, MODELS_DIR, USERS_DIR};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;
use zbus::connection::Builder;

fn warn_on_ir_misconfig(cameras: &gaze_core::config::CameraConfig) {
    let ir = cameras.ir.trim();
    if ir.is_empty() {
        if cameras.emitter_enabled {
            warn!(
                "cameras.emitter_enabled is set but cameras.ir is empty; the IR emitter will not be used"
            );
        }
        return;
    }
    if let Some(node) = gaze_core::camera::resolve_node(ir)
        && cameras.emitter_enabled
    {
        if !std::path::Path::new(&node).exists() {
            warn!(
                node = ir,
                resolved = node,
                "resolved cameras.ir device node does not exist; IR capture will fail until it appears"
            );
        }
    } else {
        warn!(
            node = ir,
            "could not resolve a physical V4L2 device node for cameras.ir; the IR emitter will not be driven"
        );
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    info!("Initializing Gaze Daemon...");

    if let Ok(uid) = daemon::get_active_session_uid().await {
        daemon::set_pipewire_runtime_for_uid(uid);
    }

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

    let recognizer_rgb = recognize::FaceRecognizer::new(rec_path.to_str().unwrap())
        .expect("Failed to load recognition model");
    let recognizer_ir = recognize::FaceRecognizer::new(rec_path.to_str().unwrap())
        .expect("Failed to load recognition model");

    let liveness_detector = if config.liveness.enabled {
        let path = models::ensure_liveness_model(MODELS_DIR)?;
        Some(liveness::LivenessDetector::new(path.to_str().unwrap())?)
    } else {
        None
    };

    let db = UserDatabase::new(USERS_DIR, config.enrollment.max_templates as usize)?;

    warn_on_ir_misconfig(&config.cameras);

    let sources = gaze_core::camera::resolve_configured_sources(&config.cameras);

    let resume_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let daemon = AuthDaemon {
        detector: Arc::new(std::sync::Mutex::new(detector)),
        recognizer_rgb: Arc::new(Mutex::new(recognizer_rgb)),
        recognizer_ir: Arc::new(Mutex::new(recognizer_ir)),
        liveness: Arc::new(Mutex::new(liveness_detector)),
        db: Arc::new(Mutex::new(db)),
        threshold: Arc::new(Mutex::new(security.threshold())),
        rgb_device: Arc::new(Mutex::new(sources.rgb)),
        ir_device: Arc::new(Mutex::new(sources.ir)),
        ir_node: Arc::new(Mutex::new(sources.ir_node)),
        emitter_enabled: Arc::new(Mutex::new(config.cameras.emitter_enabled)),
        liveness_config: Arc::new(Mutex::new(config.liveness.clone())),
        abort_if_ssh: Arc::new(Mutex::new(config.auth.abort_if_ssh)),
        abort_if_lid_closed: Arc::new(Mutex::new(config.auth.abort_if_lid_closed)),
        claim_state: Arc::new(Mutex::new(None)),
        active_cancel: Arc::new(Mutex::new(None)),
        active_extensions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        resume_pending: resume_pending.clone(),
        rt_handle: tokio::runtime::Handle::current(),
    };

    info!(elapsed = ?t_load.elapsed(), "Models & user DB loaded");

    let conn = Builder::system()?
        .name("com.gundulabs.Gaze")?
        .serve_at("/com/gundulabs/Gaze", daemon)?
        .build()
        .await?;

    tokio::spawn(daemon::watch_resume(conn.clone(), resume_pending));

    info!("Gaze Daemon listening on System Bus");
    std::future::pending::<()>().await;

    Ok(())
}
