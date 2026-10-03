// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

/// The effective value in the GDM profile, which a NixOS config sets without our override file.
pub(super) fn gdm_face_auth_from_dconf() -> Option<bool> {
    if !std::path::Path::new(GDM_DCONF_PROFILE_PATH).exists() {
        return None;
    }
    let output = std::process::Command::new("dconf")
        .arg("read")
        .arg(GDM_DCONF_FACE_AUTH_KEY)
        .env("DCONF_PROFILE", GDM_DCONF_PROFILE)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    match String::from_utf8_lossy(&output.stdout).trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub(super) fn gdm_override_error(
    action: &str,
    path: &std::path::Path,
    err: std::io::Error,
) -> fdo::Error {
    if matches!(
        err.kind(),
        std::io::ErrorKind::ReadOnlyFilesystem | std::io::ErrorKind::PermissionDenied
    ) {
        return fdo::Error::Failed(format!(
            "Failed to {action} {}: {err}. The GDM dconf database is read-only, \
             so it is managed by your system configuration rather than by Gaze; \
             on NixOS set `services.gaze.gnome.gdmFaceLogin` instead.",
            path.display()
        ));
    }
    fdo::Error::Failed(format!("Failed to {action} {}: {err}", path.display()))
}

impl AuthDaemon {
    pub(super) async fn apply_config(&self, mut new_config: Config) -> fdo::Result<()> {
        new_config.duress = self.current_config().await.duress;
        new_config
            .security
            .validate()
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        new_config
            .enrollment
            .validate()
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        new_config
            .inference
            .validate()
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        new_config
            .liveness
            .validate()
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        new_config
            .cameras
            .validate()
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;

        self.cancel_active_tasks().await;

        let new_liveness_detector = if new_config.liveness.enabled {
            let path = crate::models::ensure_liveness_model(gaze_core::config::MODELS_DIR)
                .map_err(|e| fdo::Error::Failed(format!("Failed to ensure liveness model: {e}")))?;
            Some(
                LivenessDetector::new_with_inference(path.to_str().unwrap(), &new_config.inference)
                    .map_err(|e| {
                        fdo::Error::Failed(format!("Failed to load liveness model: {e}"))
                    })?,
            )
        } else {
            None
        };

        *self.rgb_threshold.lock().await = new_config.security.rgb_threshold();
        *self.ir_threshold.lock().await = new_config.security.ir_threshold();
        *self.hybrid_policy.lock().await = new_config.security.hybrid_policy().to_string();

        let sources = resolve_configured_sources(&new_config.cameras);
        *self.rgb_device.lock().await = sources.rgb;
        *self.ir_device.lock().await = sources.ir;
        *self.ir_node.lock().await = sources.ir_node;
        *self.serial_capture.lock().await = sources.serial_capture;
        *self.emitter_enabled.lock().await = new_config.cameras.emitter_enabled;

        let mut live_cfg = self.liveness_config.lock().await;
        *live_cfg = new_config.liveness.clone();
        drop(live_cfg);

        let mut liveness_slot = self.liveness.lock().await;
        *liveness_slot = new_liveness_detector;
        drop(liveness_slot);

        let mut abort_if_ssh = self.abort_if_ssh.lock().await;
        *abort_if_ssh = new_config.auth.abort_if_ssh;

        let mut abort_if_lid_closed = self.abort_if_lid_closed.lock().await;
        *abort_if_lid_closed = new_config.auth.abort_if_lid_closed;

        let mut abort_before_first_resume = self.abort_before_first_resume.lock().await;
        *abort_before_first_resume = new_config.auth.abort_before_first_resume;

        {
            let mut db = self.db.lock().await;
            db.set_max_templates(new_config.enrollment.max_templates as usize);
        }

        let security = &new_config.security;
        info!(
            detector = security.detector(),
            recognizer = security.recognizer(),
            execution_provider = new_config.inference.execution_provider,
            device = new_config.inference.device,
            "Hot-reloading models if needed"
        );

        let (det_path, rec_path) = match crate::models::ensure_models(
            gaze_core::config::MODELS_DIR,
            security.detector(),
            security.recognizer(),
        ) {
            Ok(p) => p,
            Err(e) => return Err(fdo::Error::Failed(format!("Failed to ensure models: {e}"))),
        };

        {
            let mut detector = self.detector.lock().unwrap_or_else(|e| e.into_inner());
            match gaze_vision::detect::FaceDetector::new_with_inference(
                det_path.to_str().unwrap(),
                &new_config.inference,
            ) {
                Ok(det) => {
                    *detector = det;
                }
                Err(e) => {
                    return Err(fdo::Error::Failed(format!("Failed to load detector: {e}")));
                }
            }
        }

        {
            let mut recognizer_rgb = self.recognizer_rgb.lock().await;
            let mut recognizer_ir = self.recognizer_ir.lock().await;
            match crate::recognize::FaceRecognizer::new_with_inference(
                rec_path.to_str().unwrap(),
                &new_config.inference,
            ) {
                Ok(rec_rgb) => {
                    let rec_ir = match crate::recognize::FaceRecognizer::new_with_inference(
                        rec_path.to_str().unwrap(),
                        &new_config.inference,
                    ) {
                        Ok(r) => r,
                        Err(e) => {
                            return Err(fdo::Error::Failed(format!(
                                "Failed to load IR recognizer: {e}"
                            )));
                        }
                    };
                    *recognizer_rgb = rec_rgb;
                    *recognizer_ir = rec_ir;
                }
                Err(e) => {
                    return Err(fdo::Error::Failed(format!(
                        "Failed to load RGB recognizer: {e}"
                    )));
                }
            }
        }

        let want_encrypt = new_config.storage.encrypt_templates;
        let pending_cipher = {
            let db = self.db.lock().await;
            if want_encrypt != db.is_encrypted() {
                let dek =
                    crate::tpm::load_or_create_dek(std::path::Path::new(crate::tpm::STATE_DIR))
                        .map_err(|e| {
                            fdo::Error::Failed(format!("cannot change template encryption: {e}"))
                        })?;
                Some(crate::crypto::EmbeddingCipher::new(&dek))
            } else {
                None
            }
        };

        let save_config = || {
            new_config
                .save_to(CONFIG_PATH)
                .map_err(|e| fdo::Error::Failed(format!("Failed to save config: {e}")))
        };

        match pending_cipher {
            Some(cipher) if want_encrypt => {
                save_config()?;
                let mut db = self.db.lock().await;
                db.set_cipher(Some(cipher));
                let n = db.migrate_plaintext_to_encrypted().map_err(|e| {
                    fdo::Error::Failed(format!("failed to encrypt existing templates: {e}"))
                })?;
                info!(migrated = n, "Enabled template encryption");
            }
            Some(cipher) => {
                let mut db = self.db.lock().await;
                let n = db.decrypt_all_with(&cipher).map_err(|e| {
                    fdo::Error::Failed(format!("failed to decrypt existing templates: {e}"))
                })?;
                db.set_cipher(None);
                drop(db);
                save_config()?;
                info!(decrypted = n, "Disabled template encryption");
            }
            None => save_config()?,
        }

        info!("Config reloaded successfully");
        Ok(())
    }
}
