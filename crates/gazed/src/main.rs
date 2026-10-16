// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

mod align;
mod crypto;
mod daemon;
mod duress;
mod liveness;
pub mod models;
mod preview;
mod recognize;
mod tpm;
pub mod users;

use crate::users::UserDatabase;
use daemon::AuthDaemon;
use gaze_core::config::{CONFIG_PATH, Config, MODELS_DIR, USERS_DIR};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use zbus::connection::Builder;

fn warn_on_ir_misconfig(cameras: &gaze_core::config::CameraConfig) {
    if let Some((node, resolved)) = ir_misconfig_diagnostic(
        cameras.ir.trim(),
        cameras.emitter_enabled,
        &mut |ir| gaze_vision::camera::resolve_node(ir),
        &mut |node| std::path::Path::new(&node).exists(),
    ) {
        if let Some(resolved) = resolved {
            warn!(
                node = node,
                resolved = resolved,
                "resolved cameras.ir device node does not exist; IR capture will fail until it appears"
            );
        } else {
            warn!(
                node = node,
                "could not resolve a physical V4L2 device node for cameras.ir; the IR emitter will not be driven"
            );
        }
    } else if !cameras.ir.trim().is_empty() {
        // Resolved and present, or unresolvable with the emitter off: nothing to warn about.
    } else if cameras.emitter_enabled {
        warn!(
            "cameras.emitter_enabled is set but cameras.ir is empty; the IR emitter will not be used"
        );
    }
}

fn ir_misconfig_diagnostic(
    ir: &str,
    emitter_enabled: bool,
    resolve: &mut dyn FnMut(&str) -> Option<String>,
    exists: &mut dyn FnMut(&str) -> bool,
) -> Option<(String, Option<String>)> {
    if ir.is_empty() {
        return None;
    }
    match resolve(ir) {
        // A missing node breaks IR capture whether or not the emitter is driven.
        Some(node) => {
            if !exists(&node) {
                Some((ir.to_string(), Some(node)))
            } else {
                None
            }
        }
        // Only the emitter needs a physical V4L2 node, so stay quiet when it is off.
        None => {
            if emitter_enabled {
                Some((ir.to_string(), None))
            } else {
                None
            }
        }
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<_> = std::env::args_os().skip(1).collect();
    anyhow::ensure!(
        args.is_empty() || (args.len() == 1 && args[0] == "--migrate-legacy-templates"),
        "unknown argument; supported maintenance option: --migrate-legacy-templates"
    );
    Config::migrate_file(CONFIG_PATH);
    let config = Config::load()?;
    let _store_lock = UserDatabase::lock_store(std::path::Path::new(USERS_DIR))?;
    if let Some(warning) = gaze_security::storage::encryption_warning(
        gaze_security::storage::disk_encryption_for_paths(&[
            std::path::Path::new(USERS_DIR),
            std::path::Path::new(tpm::STATE_DIR),
            std::path::Path::new(gaze_security::keyring::STORE_DIR),
            std::path::Path::new(gaze_security::keyring::KWALLET_STORE_DIR),
        ]),
    ) {
        warn!("{warning}");
    }
    if args.len() == 1 && args[0] == "--migrate-legacy-templates" {
        let dek = tpm::load_or_create_dek(std::path::Path::new(tpm::STATE_DIR))?;
        let cipher = crypto::EmbeddingCipher::new(&dek);
        let db = UserDatabase::new_with_cipher(
            USERS_DIR,
            config.enrollment.max_templates as usize,
            None,
        )?;
        let migrated = db.migrate_legacy_with(&cipher)?;
        info!(
            migrated,
            "Migrated templates to owner-bound encryption; restart gazed"
        );
        return Ok(());
    }
    if !gaze_core::cpu::supports_inference() {
        error!(
            "{}. {}",
            gaze_core::cpu::UNSUPPORTED_CPU_MESSAGE,
            gaze_core::cpu::UNSUPPORTED_CPU_FIX
        );
        std::process::exit(i32::from(gaze_core::cpu::EXIT_UNSUPPORTED_CPU));
    }

    gaze_vision::inference::prepare_environment(&config.inference)?;
    run(config)
}

#[tokio::main]
async fn run(config: Config) -> anyhow::Result<()> {
    gaze_vision::inference::initialize_runtime(&config.inference)?;

    info!("Initializing Gaze Daemon...");

    let t_load = std::time::Instant::now();

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

    let detector = gaze_vision::detect::FaceDetector::new_with_inference(
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

    let eye_state = if config.duress.enabled {
        match daemon::load_eye_state(&config.inference) {
            Ok(classifier) => Some(classifier),
            Err(e) => {
                warn!("Duress detection is enabled but the eye-state model failed to load: {e}");
                None
            }
        }
    } else {
        None
    };

    if !config.storage.encrypt_templates {
        warn!(
            "Face-template encryption is disabled: biometrics are stored in plaintext. Enable [storage] encrypt_templates with a usable TPM, and protect state storage with full-disk encryption."
        );
    }
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
    if encrypt_templates {
        // Do not silently authenticate planted plaintext or rebind legacy ciphertext,
        // even when an older interrupted migration left a mixed store.
        db.require_bound_templates()?;
    }

    if !encrypt_templates && db.has_encrypted_templates()? {
        // Without the TPM the templates stay unreadable, but refusing to start would also
        // block the troubleshooting fix of turning encryption off when the TPM is gone.
        match tpm::load_or_create_dek(std::path::Path::new(tpm::STATE_DIR))
            .and_then(|dek| db.decrypt_all_with(&crypto::EmbeddingCipher::new(&dek)))
        {
            Ok(n) => {
                info!(
                    decrypted = n,
                    "Decrypted templates after encryption was disabled"
                );
                db.load_all()?;
            }
            Err(e) => warn!(
                "template encryption is disabled but encrypted templates remain and could not \
                 be decrypted, so those faces will not match until they are re-enrolled: {e}"
            ),
        }
    }

    let adopted = db.adopt_untagged_templates(security.recognizer())?;
    if adopted > 0 {
        info!(
            adopted,
            recognizer = security.recognizer(),
            "Recorded the recognizer for existing templates"
        );
    }
    db.use_recognizer(security.recognizer())?;

    warn_on_ir_misconfig(&config.cameras);

    let sources = gaze_vision::camera::resolve_configured_sources(&config.cameras);

    let resume_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let resume_seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (sleeping_tx, sleeping) = tokio::sync::watch::channel(false);
    let lock_epochs: daemon::LockEpochs = Arc::new(Mutex::new(std::collections::HashMap::new()));
    let claim_state: daemon::ClaimStateHandle = Arc::new(Mutex::new(None));
    let active_cancel: daemon::ActiveCancelHandle = Arc::new(Mutex::new(None));
    let pam_internal: daemon::PamInternalSets =
        Arc::new(Mutex::new(std::collections::HashMap::new()));

    let daemon = AuthDaemon {
        detector: Arc::new(std::sync::Mutex::new(detector)),
        recognizer_rgb: Arc::new(Mutex::new(recognizer_rgb)),
        recognizer_ir: Arc::new(Mutex::new(recognizer_ir)),
        liveness: Arc::new(Mutex::new(liveness_detector)),
        eye_state: Arc::new(Mutex::new(eye_state)),
        duress_lockout: Arc::new(duress::DuressLockout::new(duress::DURESS_DIR)),
        db: Arc::new(Mutex::new(db)),
        rgb_threshold: Arc::new(Mutex::new(security.rgb_threshold())),
        ir_threshold: Arc::new(Mutex::new(security.ir_threshold())),
        rgb_device: Arc::new(Mutex::new(sources.rgb)),
        ir_device: Arc::new(Mutex::new(sources.ir)),
        ir_node: Arc::new(Mutex::new(sources.ir_node)),
        serial_capture: Arc::new(Mutex::new(sources.serial_capture)),
        emitter_enabled: Arc::new(Mutex::new(config.cameras.emitter_enabled)),
        liveness_config: Arc::new(Mutex::new(config.liveness.clone())),
        hybrid_policy: Arc::new(Mutex::new(security.hybrid_policy().to_string())),
        abort_if_ssh: Arc::new(Mutex::new(config.auth.abort_if_ssh)),
        abort_if_lid_closed: Arc::new(Mutex::new(config.auth.abort_if_lid_closed)),
        abort_before_first_resume: Arc::new(Mutex::new(config.auth.abort_before_first_resume)),
        claim_state: claim_state.clone(),
        active_cancel: active_cancel.clone(),
        pam_internal: pam_internal.clone(),
        resume_pending: resume_pending.clone(),
        resume_seen: resume_seen.clone(),
        sleeping,
        lock_epochs: lock_epochs.clone(),
        benchmark_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        last_good_config: Arc::new(Mutex::new(config.clone())),
        loaded_model_config: Mutex::new(config.clone()),
        rt_handle: tokio::runtime::Handle::current(),
    };

    info!(elapsed = ?t_load.elapsed(), "Models & user DB loaded");

    let conn = Builder::system()
        .serve_at("/com/gundulabs/Gaze", daemon)
        .build()
        .await?;

    tokio::spawn(daemon::watch_resume(
        conn.clone(),
        resume_pending,
        resume_seen,
        sleeping_tx,
    ));
    tokio::spawn(daemon::watch_session_locks(conn.clone(), lock_epochs));

    // No client can claim before the well-known name exists, so subscribing here is what makes
    // the watcher airtight. Fatal by design, so systemd restarts rather than stranding claims.
    let claim_owners = daemon::subscribe_claim_owners(&conn).await?;
    tokio::spawn(daemon::watch_claim_owner(
        claim_owners,
        claim_state,
        active_cancel,
        pam_internal,
    ));

    conn.request_name("com.gundulabs.Gaze").await?;

    info!("Gaze Daemon listening on System Bus");
    std::future::pending::<()>().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::ir_misconfig_diagnostic;

    fn no_resolve(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn empty_ir_never_warns_from_the_diagnostic() {
        for emitter in [false, true] {
            assert_eq!(
                ir_misconfig_diagnostic("", emitter, &mut no_resolve, &mut |_| true,),
                None,
                "empty ir is the caller's empty-emitter branch, not this one"
            );
        }
    }

    #[test]
    fn missing_node_warns_with_or_without_the_emitter() {
        for emitter in [false, true] {
            assert_eq!(
                ir_misconfig_diagnostic(
                    "/dev/video2",
                    emitter,
                    &mut |ir| Some(format!("/dev/{ir}")).map(|_| "/dev/video2".to_string()),
                    &mut |_| false,
                ),
                Some(("/dev/video2".to_string(), Some("/dev/video2".to_string())))
            );
        }
    }

    #[test]
    fn present_node_is_quiet() {
        assert_eq!(
            ir_misconfig_diagnostic(
                "/dev/video2",
                true,
                &mut |_| Some("/dev/video2".to_string()),
                &mut |_| true,
            ),
            None
        );
    }

    #[test]
    fn unresolvable_source_warns_only_when_the_emitter_needs_it() {
        assert_eq!(
            ir_misconfig_diagnostic(
                "pipewiresrc target-object=x",
                true,
                &mut no_resolve,
                &mut |_| true
            ),
            Some(("pipewiresrc target-object=x".to_string(), None))
        );
        assert_eq!(
            ir_misconfig_diagnostic(
                "pipewiresrc target-object=x",
                false,
                &mut no_resolve,
                &mut |_| true
            ),
            None
        );
    }
}
