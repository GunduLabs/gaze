// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[derive(Debug, PartialEq, Eq)]
struct ModelReloads {
    detector: bool,
    recognizer: bool,
    liveness: bool,
    inference: bool,
}

impl ModelReloads {
    fn between(loaded: &Config, requested: &Config) -> Self {
        let inference = loaded.inference.execution_provider
            != requested.inference.execution_provider
            || loaded.inference.device != requested.inference.device;
        Self {
            detector: inference || loaded.security.detector() != requested.security.detector(),
            recognizer: inference
                || loaded.security.recognizer() != requested.security.recognizer(),
            liveness: loaded.liveness.enabled != requested.liveness.enabled
                || (inference && requested.liveness.enabled),
            inference,
        }
    }
}

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
        let mut loaded_model_config = self.loaded_model_config.lock().await;
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

        let reload = ModelReloads::between(&loaded_model_config, &new_config);
        let security = &new_config.security;
        info!(
            ?reload,
            detector = security.detector(),
            recognizer = security.recognizer(),
            execution_provider = new_config.inference.execution_provider,
            device = new_config.inference.device,
            "Applying model settings"
        );

        // Prepare every replacement before touching the running sessions. A failed
        // load must leave both the sessions and their remembered settings intact.
        let paths = if reload.detector || reload.recognizer {
            Some(
                crate::models::ensure_models(
                    gaze_core::config::MODELS_DIR,
                    security.detector(),
                    security.recognizer(),
                )
                .map_err(|e| fdo::Error::Failed(format!("Failed to ensure models: {e}")))?,
            )
        } else {
            None
        };
        let new_detector = if reload.detector {
            let (path, _) = paths
                .as_ref()
                .expect("detector reload requires model paths");
            Some(
                FaceDetector::new_with_inference(path.to_str().unwrap(), &new_config.inference)
                    .map_err(|e| fdo::Error::Failed(format!("Failed to load detector: {e}")))?,
            )
        } else {
            None
        };
        let new_recognizers = if reload.recognizer {
            let (_, path) = paths
                .as_ref()
                .expect("recognizer reload requires model paths");
            let rgb = FaceRecognizer::new_with_inference(
                path.to_str().unwrap(),
                &new_config.inference,
            )
            .map_err(|e| fdo::Error::Failed(format!("Failed to load RGB recognizer: {e}")))?;
            let ir =
                FaceRecognizer::new_with_inference(path.to_str().unwrap(), &new_config.inference)
                    .map_err(|e| fdo::Error::Failed(format!("Failed to load IR recognizer: {e}")))?;
            Some((rgb, ir))
        } else {
            None
        };
        let new_liveness_detector = if reload.liveness && new_config.liveness.enabled {
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

        if let Some(detector) = new_detector {
            *self.detector.lock().unwrap_or_else(|e| e.into_inner()) = detector;
        }
        if let Some((rgb, ir)) = new_recognizers {
            let mut recognizer_rgb = self.recognizer_rgb.lock().await;
            let mut recognizer_ir = self.recognizer_ir.lock().await;
            *recognizer_rgb = rgb;
            *recognizer_ir = ir;
        }
        if reload.liveness {
            *self.liveness.lock().await = new_liveness_detector;
        }
        if reload.inference {
            // The optional eye-state session is loaded lazily by verification.
            *self.eye_state.lock().await = None;
        }
        // Remember what is actually loaded even if a later storage/config write
        // fails, so the next request compares against the live sessions.
        *loaded_model_config = new_config.clone();

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

#[cfg(test)]
mod tests {
    use super::*;
    use gaze_core::config::SecurityLevel;

    fn assert_reloads(loaded: &Config, requested: &Config, expected: (bool, bool, bool, bool)) {
        let reload = ModelReloads::between(loaded, requested);
        assert_eq!(
            (
                reload.detector,
                reload.recognizer,
                reload.liveness,
                reload.inference
            ),
            expected
        );
    }

    #[test]
    fn ordinary_settings_updates_reuse_all_sessions() {
        let loaded = Config::default();
        let mut requested = loaded.clone();
        requested.security =
            SecurityLevel::custom("standard".into(), "standard".into(), 0.6, "and".into());
        requested.cameras.rgb = "/dev/video4".into();
        requested.cameras.emitter_enabled = !loaded.cameras.emitter_enabled;
        requested.auth.abort_if_ssh = !loaded.auth.abort_if_ssh;
        requested.enrollment.max_templates += 1;
        requested.liveness.threshold = 0.9;
        requested.liveness.max_seconds = 3.0;
        requested.storage.encrypt_templates = !loaded.storage.encrypt_templates;
        assert_reloads(&loaded, &requested, (false, false, false, false));
        assert_reloads(&loaded, &loaded, (false, false, false, false));
    }

    #[test]
    fn presets_that_resolve_to_the_same_models_reuse_sessions() {
        for (before, after) in [
            (SecurityLevel::low(), SecurityLevel::medium()),
            (SecurityLevel::high(), SecurityLevel::maximum()),
        ] {
            let loaded = Config {
                security: before,
                ..Config::default()
            };
            let mut requested = loaded.clone();
            requested.security = after;
            assert_reloads(&loaded, &requested, (false, false, false, false));
        }
    }

    #[test]
    fn changing_one_model_reloads_only_that_component() {
        let loaded = Config::default();
        for (detector, recognizer, expected) in [
            ("accurate", "standard", (true, false, false, false)),
            ("standard", "accurate", (false, true, false, false)),
            ("accurate", "accurate", (true, true, false, false)),
        ] {
            let mut requested = loaded.clone();
            requested.security =
                SecurityLevel::custom(detector.into(), recognizer.into(), 0.4, "and".into());
            assert_reloads(&loaded, &requested, expected);
        }
    }

    #[test]
    fn toggling_liveness_changes_only_its_session() {
        let enabled = Config::default();
        let mut disabled = enabled.clone();
        disabled.liveness.enabled = false;
        assert_reloads(&enabled, &disabled, (false, false, true, false));
        assert_reloads(&disabled, &enabled, (false, false, true, false));
        assert_reloads(&disabled, &disabled, (false, false, false, false));
    }

    #[test]
    fn provider_and_device_changes_reload_all_enabled_sessions() {
        let mut openvino = Config::default();
        openvino.inference.execution_provider = "openvino".into();
        for (provider, device) in [("cpu", "cpu"), ("openvino", "gpu")] {
            let mut requested = openvino.clone();
            requested.inference.execution_provider = provider.into();
            requested.inference.device = device.into();
            requested.inference.validate().unwrap();
            assert_reloads(&openvino, &requested, (true, true, true, true));
            let mut disabled = openvino.clone();
            disabled.liveness.enabled = false;
            requested.liveness.enabled = false;
            assert_reloads(&disabled, &requested, (true, true, false, true));
        }
    }

    #[test]
    fn reading_edited_disk_settings_does_not_hide_a_needed_reload() {
        let loaded = Config::default();
        let mut on_disk = loaded.clone();
        on_disk.security = SecurityLevel::high();
        let mut last_good = loaded.clone();
        let requested = resolve_config(Ok(on_disk), &mut last_good);
        assert_eq!(last_good.security.level, "high");
        assert_reloads(&loaded, &requested, (true, true, false, false));
    }
}
