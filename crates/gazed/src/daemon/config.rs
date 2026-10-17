// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use tokio::sync::{OwnedRwLockReadGuard, RwLock};

/// Applied settings; read guards pin their model generation through teardown.
pub struct ConfigState {
    current: Arc<RwLock<Config>>,
    updates: Arc<Mutex<()>>,
}

impl ConfigState {
    pub fn new(mut config: Config) -> Self {
        if config.clamp_keyring() {
            warn!(
                path = CONFIG_PATH,
                "keyring unlock needs encryption and liveness; ignoring the keyring options"
            );
        }
        Self {
            current: Arc::new(RwLock::new(config)),
            updates: Arc::new(Mutex::new(())),
        }
    }

    pub(super) async fn read(&self) -> OwnedRwLockReadGuard<Config> {
        self.current.clone().read_owned().await
    }

    /// Register cancellation before taking this guard; keep it through capture teardown.
    pub(super) async fn for_run(
        &self,
        cancel: &mut oneshot::Receiver<()>,
    ) -> fdo::Result<OwnedRwLockReadGuard<Config>> {
        let config = tokio::select! {
            biased;
            _ = &mut *cancel => return Err(fdo::Error::Failed("capture cancelled".into())),
            config = self.read() => config,
        };
        if cancel.try_recv() != Err(oneshot::error::TryRecvError::Empty) {
            return Err(fdo::Error::Failed("capture cancelled".into()));
        }
        Ok(config)
    }

    async fn apply<P: Send + 'static>(
        &self,
        active_cancel: &ActiveCancelHandle,
        prepare: impl FnOnce(Config) -> fdo::Result<(Config, P)> + Send + 'static,
        commit: impl FnOnce(&Config, P) -> fdo::Result<()> + Send + 'static,
    ) -> fdo::Result<()> {
        let update = self.updates.clone().lock_owned().await;
        let current = self.read().await.clone();
        // Retain serialization if cancellation leaves the preparation worker running.
        let (requested, prepared, update) = tokio::task::spawn_blocking(move || {
            let (requested, prepared) = prepare(current)?;
            Ok::<_, fdo::Error>((requested, prepared, update))
        })
        .await
        .map_err(|e| fdo::Error::Failed(format!("Config preparation task failed: {e}")))??;

        // Queue the writer before cancellation so new runs cannot enter the old snapshot.
        let write = self.current.clone().write_owned();
        tokio::pin!(write);
        let current = match futures::poll!(&mut write) {
            std::task::Poll::Ready(current) => current,
            std::task::Poll::Pending => {
                if let Some(cancel) = active_cancel.lock().await.take() {
                    let _ = cancel.send(());
                }
                write.await
            }
        };

        tokio::task::spawn_blocking(move || {
            // Keep both locks in the worker even if the requesting D-Bus task is dropped.
            let _update = update;
            let mut current = current;
            commit(&requested, prepared)?;
            *current = requested;
            Ok(())
        })
        .await
        .map_err(|e| fdo::Error::Failed(format!("Config commit task failed: {e}")))?
    }
}

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

pub(super) fn write_gdm_face_auth(enabled: bool) -> fdo::Result<bool> {
    let path = std::path::Path::new(GDM_FACE_OVERRIDE_PATH);
    // Already in the requested state elsewhere, so don't write a read-only /etc.
    if !path.exists() && gdm_face_auth_from_dconf() == Some(enabled) {
        info!(enabled, "GDM face authentication already set outside Gaze");
        return Ok(enabled);
    }

    if enabled {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| gdm_override_error("create", parent, e))?;
        }
        std::fs::write(path, GDM_DCONF_OVERRIDE_CONTENT)
            .map_err(|e| gdm_override_error("write", path, e))?;
    } else if path.exists() {
        std::fs::remove_file(path).map_err(|e| gdm_override_error("remove", path, e))?;
    }

    let status = std::process::Command::new("dconf")
        .arg("update")
        .status()
        .map_err(|e| fdo::Error::Failed(format!("Failed to run dconf update: {e}")))?;
    if !status.success() {
        return Err(fdo::Error::Failed(format!(
            "dconf update exited with status {}",
            status.code().unwrap_or(-1)
        )));
    }

    // GDM's own user db outranks the system db the override lives in.
    if let Some(effective) = gdm_face_auth_from_dconf()
        && effective != enabled
    {
        let source = gdm_greeter_dconf_source(GDM_DCONF_FACE_AUTH_KEY)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "another dconf system-db".to_string());
        return Err(fdo::Error::Failed(format!(
            "Updated the Gaze override, but GDM still resolves face login as {effective} \
             because {source} sets it; clear the key there"
        )));
    }

    info!(enabled, "Updated GDM face authentication override");
    Ok(enabled)
}

struct PreparedConfig {
    reload: ModelReloads,
    models: ModelSessions,
    liveness: Option<LivenessDetector>,
    cipher: Option<crate::crypto::EmbeddingCipher>,
}

impl PreparedConfig {
    fn load(
        current: &Config,
        requested: &Config,
        current_models: &ModelSessions,
    ) -> fdo::Result<Self> {
        let reload = ModelReloads::between(current, requested);
        let add_ir = ir_models_needed(requested) && current_models.ir.is_none();
        let mut models = current_models.clone();
        if !ir_models_needed(requested) {
            models.ir = None;
        }
        let security = &requested.security;
        info!(
            ?reload,
            detector = security.detector(),
            recognizer = security.recognizer(),
            execution_provider = requested.inference.execution_provider,
            device = requested.inference.device,
            "Preparing model settings"
        );
        let paths = if reload.detector || reload.recognizer || add_ir {
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
        if let Some((detector, recognizer)) = paths.as_ref() {
            for (spectrum, capture) in std::iter::once(("RGB", &mut models.rgb))
                .chain(models.ir.iter_mut().map(|capture| ("IR", capture)))
            {
                if reload.detector {
                    capture.detector = Arc::new(std::sync::Mutex::new(
                        FaceDetector::new_with_inference(detector, &requested.inference).map_err(
                            |e| {
                                fdo::Error::Failed(format!(
                                    "Failed to load {spectrum} detector: {e}"
                                ))
                            },
                        )?,
                    ));
                }
                if reload.recognizer {
                    capture.recognizer = Arc::new(Mutex::new(
                        FaceRecognizer::new_with_inference(recognizer, &requested.inference)
                            .map_err(|e| {
                                fdo::Error::Failed(format!(
                                    "Failed to load {spectrum} recognizer: {e}"
                                ))
                            })?,
                    ));
                }
            }
            if add_ir {
                models.ir = Some(
                    CaptureModels::load(detector, recognizer, &requested.inference).map_err(
                        |e| fdo::Error::Failed(format!("Failed to load IR models: {e:#}")),
                    )?,
                );
            }
        }
        let liveness = if reload.liveness && requested.liveness.enabled {
            let path = crate::models::ensure_liveness_model(gaze_core::config::MODELS_DIR)
                .map_err(|e| fdo::Error::Failed(format!("Failed to ensure liveness model: {e}")))?;
            Some(
                LivenessDetector::new_with_inference(&path, &requested.inference).map_err(|e| {
                    fdo::Error::Failed(format!("Failed to load liveness model: {e}"))
                })?,
            )
        } else {
            None
        };
        let cipher = if current.storage.encrypt_templates != requested.storage.encrypt_templates {
            let state_dir = std::path::Path::new(crate::tpm::STATE_DIR);
            let dek = crate::tpm::load_or_create_dek(state_dir).map_err(|e| {
                fdo::Error::Failed(format!("cannot change template encryption: {e}"))
            })?;
            Some(crate::crypto::EmbeddingCipher::new(&dek))
        } else {
            None
        };
        Ok(Self {
            reload,
            models,
            liveness,
            cipher,
        })
    }
}

impl AuthDaemon {
    pub(super) async fn apply_config(
        &self,
        update: impl FnOnce(Config) -> fdo::Result<Config> + Send + 'static,
    ) -> fdo::Result<()> {
        let db = self.db.clone();
        let models = self.models.clone();
        let current_models = models.clone();
        let liveness = self.liveness.clone();
        let eye_state = self.eye_state.clone();
        self.settings
            .apply(
                &self.active_cancel,
                move |current| {
                    let mut requested = update(current.clone())?;
                    requested.duress = current.duress.clone();
                    requested
                        .security
                        .validate()
                        .and_then(|_| requested.enrollment.validate())
                        .and_then(|_| requested.inference.validate())
                        .and_then(|_| requested.liveness.validate())
                        .and_then(|_| requested.cameras.validate())
                        .and_then(|_| requested.storage.validate_keyring(&requested.liveness))
                        .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
                    let loaded = current_models
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .clone();
                    let prepared = PreparedConfig::load(&current, &requested, &loaded)?;
                    Ok((requested, prepared))
                },
                move |requested, prepared| {
                    let migrated = db
                        .blocking_lock()
                        .apply_settings(
                            prepared.cipher,
                            requested.storage.encrypt_templates,
                            prepared
                                .reload
                                .recognizer
                                .then_some(requested.security.recognizer()),
                            requested.enrollment.max_templates as usize,
                            || requested.save_to(CONFIG_PATH),
                        )
                        .map_err(|e| {
                            fdo::Error::Failed(format!(
                                "Failed to apply template storage settings: {e}"
                            ))
                        })?;
                    if migrated > 0 {
                        info!(
                            migrated,
                            encrypted = requested.storage.encrypt_templates,
                            "Changed template encryption"
                        );
                    }

                    // No fallible work remains, and every model user has finished teardown.
                    *models.lock().unwrap_or_else(|error| error.into_inner()) = prepared.models;
                    if prepared.reload.liveness {
                        *liveness.blocking_lock() = prepared.liveness;
                    }
                    if prepared.reload.inference {
                        *eye_state.blocking_lock() = None;
                    }
                    info!("Config reloaded successfully");
                    Ok(())
                },
            )
            .await
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

    #[tokio::test]
    async fn config_warnings_are_not_repeated_by_property_reads() {
        use std::io::{Read, Seek};

        let output = Arc::new(tempfile::tempfile().unwrap());
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(output.clone())
            .finish();
        let _subscriber = tracing::subscriber::set_default(subscriber);
        let mut config = Config::default();
        config.storage.encrypt_templates = false;
        config.storage.unlock_gnome_keyring = true;
        let settings = ConfigState::new(config);
        for _ in 0..3 {
            assert!(!settings.read().await.storage.unlock_gnome_keyring);
        }
        let mut reader = output.as_ref();
        reader.rewind().unwrap();
        let mut logs = String::new();
        reader.read_to_string(&mut logs).unwrap();
        assert_eq!(logs.matches("ignoring the keyring options").count(), 1);
    }

    #[tokio::test]
    async fn model_reloads_are_computed_from_the_applied_snapshot() {
        let settings = ConfigState::new(Config::default());
        let loaded = settings.read().await;
        let mut requested = loaded.clone();
        requested.security = SecurityLevel::high();
        assert_reloads(&loaded, &requested, (true, true, false, false));
        assert_eq!(settings.read().await.security.level, "medium");
    }

    #[tokio::test]
    async fn failed_preparation_leaves_the_capture_and_config_untouched() {
        let settings = ConfigState::new(Config::default());
        let (tx, mut rx) = oneshot::channel();
        let cancel = Arc::new(Mutex::new(Some(tx)));
        let run = settings.for_run(&mut rx).await.unwrap();
        let committed = Arc::new(AtomicBool::new(false));
        let marker = committed.clone();
        let result = settings
            .apply(
                &cancel,
                |_| Err::<(Config, ()), _>(fdo::Error::Failed("model load failed".into())),
                move |_, ()| {
                    marker.store(true, Ordering::SeqCst);
                    Ok(())
                },
            )
            .await;
        assert!(result.is_err());
        assert!(!committed.load(Ordering::SeqCst));
        assert_eq!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty));
        assert_eq!(run.security.level, "medium");
        assert_eq!(settings.read().await.security.level, "medium");
    }

    #[tokio::test]
    async fn updates_cancel_capture_but_wait_for_teardown_before_swapping_models() {
        let settings = Arc::new(ConfigState::new(Config::default()));
        let (tx, mut rx) = oneshot::channel();
        let cancel = Arc::new(Mutex::new(Some(tx)));
        let run = settings.for_run(&mut rx).await.unwrap();
        let committed = Arc::new(AtomicBool::new(false));
        let marker = committed.clone();
        let writer = settings.clone();
        let updating = tokio::spawn(async move {
            writer
                .apply(
                    &cancel,
                    |mut config| {
                        config.security = SecurityLevel::high();
                        Ok((config, ()))
                    },
                    move |_, ()| {
                        marker.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
        });
        rx.await.unwrap();
        assert!(!committed.load(Ordering::SeqCst));
        assert_eq!(run.security.level, "medium");

        let (_next_tx, mut next_rx) = oneshot::channel();
        let next_run = settings.for_run(&mut next_rx);
        tokio::pin!(next_run);
        assert!(futures::poll!(&mut next_run).is_pending());
        drop(run);
        updating.await.unwrap().unwrap();
        assert!(committed.load(Ordering::SeqCst));
        assert_eq!(next_run.await.unwrap().security.level, "high");
    }

    #[tokio::test]
    async fn a_run_waiting_for_config_can_be_cancelled() {
        let settings = ConfigState::new(Config::default());
        let writer = settings.current.write().await;
        let (tx, mut rx) = oneshot::channel();
        let run = settings.for_run(&mut rx);
        tokio::pin!(run);
        assert!(futures::poll!(&mut run).is_pending());
        tx.send(()).unwrap();
        assert!(run.await.is_err());
        drop(writer);
        assert!(settings.current.try_write().is_ok());
    }

    #[tokio::test]
    async fn concurrent_updates_build_on_the_last_successful_commit() {
        let settings = ConfigState::new(Config::default());
        let cancel = Arc::new(Mutex::new(None));
        let (camera, liveness) = tokio::join!(
            settings.apply(
                &cancel,
                |mut config| {
                    config.cameras.rgb = "/dev/video8".into();
                    Ok((config, ()))
                },
                |_, ()| Ok(()),
            ),
            settings.apply(
                &cancel,
                |mut config| {
                    config.liveness.threshold = 0.95;
                    Ok((config, ()))
                },
                |_, ()| Ok(()),
            ),
        );
        camera.unwrap();
        liveness.unwrap();
        let current = settings.read().await;
        assert_eq!(current.cameras.rgb, "/dev/video8");
        assert_eq!(current.liveness.threshold, 0.95);
    }

    #[tokio::test]
    async fn blocking_preparation_keeps_the_runtime_and_old_capture_available() {
        let settings = Arc::new(ConfigState::new(Config::default()));
        let (cancel_tx, mut cancel_rx) = oneshot::channel();
        let cancel = Arc::new(Mutex::new(Some(cancel_tx)));
        let run = settings.for_run(&mut cancel_rx).await.unwrap();
        let (started_tx, started_rx) = oneshot::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let writer = settings.clone();
        let updating = tokio::spawn(async move {
            writer
                .apply(
                    &cancel,
                    move |mut config| {
                        started_tx.send(()).unwrap();
                        finish_rx.blocking_recv().unwrap();
                        config.liveness.threshold = 0.95;
                        Ok((config, ()))
                    },
                    |_, ()| Ok(()),
                )
                .await
        });
        started_rx.await.unwrap();
        assert_eq!(
            settings.read().await.liveness.threshold,
            run.liveness.threshold
        );
        assert_eq!(
            cancel_rx.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
        finish_tx.send(()).unwrap();
        cancel_rx.await.unwrap();
        drop(run);
        updating.await.unwrap().unwrap();
        assert_eq!(settings.read().await.liveness.threshold, 0.95);
    }

    #[tokio::test]
    async fn cancelled_preparation_stays_serialized_until_its_worker_finishes() {
        let settings = Arc::new(ConfigState::new(Config::default()));
        let cancel = Arc::new(Mutex::new(None));
        let (started_tx, started_rx) = oneshot::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let writer = settings.clone();
        let first_cancel = cancel.clone();
        let updating = tokio::spawn(async move {
            writer
                .apply(
                    &first_cancel,
                    move |mut config| {
                        started_tx.send(()).unwrap();
                        finish_rx.blocking_recv().unwrap();
                        config.security = SecurityLevel::high();
                        Ok((config, ()))
                    },
                    |_, ()| panic!("an abandoned preparation must not commit"),
                )
                .await
        });
        started_rx.await.unwrap();
        updating.abort();
        assert!(updating.await.unwrap_err().is_cancelled());
        assert!(settings.updates.try_lock().is_err());
        let next = settings.apply(
            &cancel,
            |mut config| {
                config.liveness.threshold = 0.95;
                Ok((config, ()))
            },
            |_, ()| Ok(()),
        );
        tokio::pin!(next);
        assert!(futures::poll!(&mut next).is_pending());
        assert_eq!(settings.read().await.security.level, "medium");
        finish_tx.send(()).unwrap();
        next.await.unwrap();
        let current = settings.read().await;
        assert_eq!(current.security.level, "medium");
        assert_eq!(current.liveness.threshold, 0.95);
    }

    #[tokio::test]
    async fn a_blocking_commit_pins_settings_even_after_the_request_is_dropped() {
        let settings = Arc::new(ConfigState::new(Config::default()));
        let cancel = Arc::new(Mutex::new(None));
        let (started_tx, started_rx) = oneshot::channel();
        let (finish_tx, finish_rx) = oneshot::channel();
        let writer = settings.clone();
        let updating = tokio::spawn(async move {
            writer
                .apply(
                    &cancel,
                    |mut config| {
                        config.liveness.threshold = 0.95;
                        Ok((config, ()))
                    },
                    move |_, ()| {
                        started_tx.send(()).unwrap();
                        finish_rx.blocking_recv().unwrap();
                        Ok(())
                    },
                )
                .await
        });
        started_rx.await.unwrap();
        updating.abort();
        assert!(updating.await.unwrap_err().is_cancelled());
        let read = settings.read();
        tokio::pin!(read);
        assert!(futures::poll!(&mut read).is_pending());
        tokio::time::sleep(Duration::from_millis(1)).await;
        finish_tx.send(()).unwrap();
        assert_eq!(read.await.liveness.threshold, 0.95);
    }

    #[tokio::test]
    async fn failed_persistence_keeps_the_snapshot_and_allows_a_retry() {
        let settings = ConfigState::new(Config::default());
        let cancel = Arc::new(Mutex::new(None));
        let before = settings.read().await.to_toml().unwrap();
        let result = settings
            .apply(
                &cancel,
                |mut config| {
                    config.security = SecurityLevel::high();
                    Ok((config, ()))
                },
                |_, ()| Err(fdo::Error::Failed("read-only config file".into())),
            )
            .await;
        assert!(result.is_err());
        assert_eq!(settings.read().await.to_toml().unwrap(), before);
        settings
            .apply(
                &cancel,
                |mut config| {
                    config.security = SecurityLevel::high();
                    Ok((config, ()))
                },
                |_, ()| Ok(()),
            )
            .await
            .unwrap();
        assert_eq!(settings.read().await.security.level, "high");
    }
}
