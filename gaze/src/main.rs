// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

mod align;
mod crypto;
mod daemon;
mod liveness;
pub mod models;
mod preview;
mod recognize;
mod tpm;
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
    match gaze_core::camera::resolve_node(ir) {
        // A missing node breaks IR capture whether or not the emitter is driven.
        Some(node) => {
            if !std::path::Path::new(&node).exists() {
                warn!(
                    node = ir,
                    resolved = node,
                    "resolved cameras.ir device node does not exist; IR capture will fail until it appears"
                );
            }
        }
        // Only the emitter needs a physical V4L2 node, so stay quiet when it is off.
        None => {
            if cameras.emitter_enabled {
                warn!(
                    node = ir,
                    "could not resolve a physical V4L2 device node for cameras.ir; the IR emitter will not be driven"
                );
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    gaze_core::inference::ensure_supported_runtime()?;
    if let Ok(version) = gaze_core::inference::runtime_version() {
        info!(version, "Loaded ONNX Runtime");
    }

    let _initialized = ort::init()
        .with_name("gazed")
        .with_logger(std::sync::Arc::new(
            move |level, category, _id, _location, message| {
                if tracing::enabled!(tracing::Level::DEBUG) {
                    tracing::debug!(target: "ort", "[{}] ({:?}) {}", category, level, message);
                }
            },
        ))
        .commit();

    info!("Initializing Gaze Daemon...");

    if let Ok(uid) = daemon::get_active_session_uid().await {
        daemon::bind_pipewire_session_for_uid(uid);
    }

    let t_load = std::time::Instant::now();

    let config = Config::load()?;
    let security = &config.security;

    info!(
        level = ?security,
        detector = security.detector(),
        recognizer = security.recognizer(),
        rgb_threshold = security.rgb_threshold(),
        ir_threshold = security.ir_threshold(),
        "Loaded security config"
    );
    info!(
        execution_provider = config.inference.execution_provider,
        device = config.inference.device,
        "Loaded inference config"
    );

    let (det_path, rec_path) =
        models::ensure_models(MODELS_DIR, security.detector(), security.recognizer())?;

    let detector = gaze_core::detect::FaceDetector::new_with_inference(
        det_path.to_str().unwrap(),
        &config.inference,
    )
    .expect("Failed to load detection model");

    let recognizer_rgb = recognize::FaceRecognizer::new_with_inference(
        rec_path.to_str().unwrap(),
        &config.inference,
    )
    .expect("Failed to load recognition model");
    let recognizer_ir = recognize::FaceRecognizer::new_with_inference(
        rec_path.to_str().unwrap(),
        &config.inference,
    )
    .expect("Failed to load recognition model");

    let liveness_detector = if config.liveness.enabled {
        let path = models::ensure_liveness_model(MODELS_DIR)?;
        Some(liveness::LivenessDetector::new_with_inference(
            path.to_str().unwrap(),
            &config.inference,
        )?)
    } else {
        None
    };

    let cipher = if config.storage.encrypt_templates {
        let dek = tpm::load_or_create_dek(std::path::Path::new(tpm::STATE_DIR)).map_err(|e| {
            anyhow::anyhow!(
                "template encryption is enabled ([storage] encrypt_templates) but no usable TPM \
                 is available, so the daemon is refusing to start rather than write unprotected \
                 biometric data: {e}"
            )
        })?;
        info!("Template encryption enabled (AES-256-GCM under a TPM-sealed key)");
        Some(crypto::EmbeddingCipher::new(&dek))
    } else {
        None
    };

    let encrypt_templates = config.storage.encrypt_templates;
    let mut db =
        UserDatabase::new_with_cipher(USERS_DIR, config.enrollment.max_templates as usize, cipher)?;
    if encrypt_templates && !db.has_encrypted_templates()? {
        match db.migrate_plaintext_to_encrypted() {
            Ok(0) => {}
            Ok(n) => {
                info!(
                    migrated = n,
                    "Encrypted existing plaintext templates at rest"
                );
                db.load_all()?;
            }
            Err(e) => warn!("Could not migrate some plaintext templates to encrypted: {e}"),
        }
    }

    warn_on_ir_misconfig(&config.cameras);

    let sources = gaze_core::camera::resolve_configured_sources(&config.cameras);

    let resume_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let lock_epochs: daemon::LockEpochs = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let claim_state: daemon::ClaimStateHandle = Arc::new(Mutex::new(None));
    let active_cancel: daemon::ActiveCancelHandle = Arc::new(Mutex::new(None));

    let daemon = AuthDaemon {
        detector: Arc::new(std::sync::Mutex::new(detector)),
        recognizer_rgb: Arc::new(Mutex::new(recognizer_rgb)),
        recognizer_ir: Arc::new(Mutex::new(recognizer_ir)),
        liveness: Arc::new(Mutex::new(liveness_detector)),
        db: Arc::new(Mutex::new(db)),
        rgb_threshold: Arc::new(Mutex::new(security.rgb_threshold())),
        ir_threshold: Arc::new(Mutex::new(security.ir_threshold())),
        rgb_device: Arc::new(Mutex::new(sources.rgb)),
        ir_device: Arc::new(Mutex::new(sources.ir)),
        ir_node: Arc::new(Mutex::new(sources.ir_node)),
        emitter_enabled: Arc::new(Mutex::new(config.cameras.emitter_enabled)),
        liveness_config: Arc::new(Mutex::new(config.liveness.clone())),
        hybrid_policy: Arc::new(Mutex::new(security.hybrid_policy().to_string())),
        abort_if_ssh: Arc::new(Mutex::new(config.auth.abort_if_ssh)),
        abort_if_lid_closed: Arc::new(Mutex::new(config.auth.abort_if_lid_closed)),
        claim_state: claim_state.clone(),
        active_cancel: active_cancel.clone(),
        active_extensions: Arc::new(Mutex::new(std::collections::HashMap::new())),
        resume_pending: resume_pending.clone(),
        lock_epochs: lock_epochs.clone(),
        benchmark_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        last_good_config: Arc::new(Mutex::new(config.clone())),
        rt_handle: tokio::runtime::Handle::current(),
    };

    info!(elapsed = ?t_load.elapsed(), "Models & user DB loaded");

    let conn = Builder::system()?
        .serve_at("/com/gundulabs/Gaze", daemon)?
        .build()
        .await?;

    tokio::spawn(daemon::watch_resume(conn.clone(), resume_pending));
    tokio::spawn(daemon::watch_session_locks(conn.clone(), lock_epochs));

    // No client can claim before the well-known name exists, so subscribing here is what makes
    // the watcher airtight. Fatal by design, so systemd restarts rather than stranding claims.
    let claim_owners = daemon::subscribe_claim_owners(&conn).await?;
    tokio::spawn(daemon::watch_claim_owner(
        claim_owners,
        claim_state,
        active_cancel,
    ));

    conn.request_name("com.gundulabs.Gaze").await?;

    info!("Gaze Daemon listening on System Bus");
    std::future::pending::<()>().await;

    Ok(())
}
