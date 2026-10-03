// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) enum VerifyMsg {
    PhaseStarted(Spectrum),
    Diagnostic(String),
    Status(Spectrum, CaptureStatus, Option<ndarray::Array1<f32>>, f64),
    Success(Spectrum, ndarray::Array1<f32>),
    Duress(Spectrum),
    Error(String),
}

pub(super) fn should_yield_rgb_to_ir(policy: &str, run_ir: bool, status: CaptureStatus) -> bool {
    run_ir && !matches!(policy, "or" | "and") && matches!(status, CaptureStatus::TooDark)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VerifyGiveUp {
    NoFace,
    NoUsableFrame,
}

impl VerifyGiveUp {
    fn reason(self) -> String {
        match self {
            Self::NoFace => format!(
                "giving up after {}s without a detected face",
                VERIFY_NO_FACE_TIMEOUT.as_secs()
            ),
            Self::NoUsableFrame => format!(
                "giving up after {}s without a usable frame",
                VERIFY_NO_USABLE_TIMEOUT.as_secs()
            ),
        }
    }
}

/// Whether a verify run has spent either deadline. Both are needed: `Clipped` and `Ready` refresh
/// `since_face` and never `since_usable`, so alone they keep a run alive with nothing to decide it.
pub(super) fn verify_give_up(since_face: Duration, since_usable: Duration) -> Option<VerifyGiveUp> {
    if since_face >= VERIFY_NO_FACE_TIMEOUT {
        return Some(VerifyGiveUp::NoFace);
    }
    if since_usable >= VERIFY_NO_USABLE_TIMEOUT {
        return Some(VerifyGiveUp::NoUsableFrame);
    }
    None
}

pub(super) fn ir_waits_for_rgb(run_rgb: bool, serial_capture: bool) -> bool {
    run_rgb && serial_capture
}

pub(super) fn rgb_yields_camera_on_budget(run_ir: bool, serial_capture: bool) -> bool {
    run_ir && serial_capture
}

pub(super) fn hybrid_auth_passed(
    policy: &str,
    run_rgb: bool,
    run_ir: bool,
    rgb_attempted: bool,
    rgb_status: CaptureStatus,
    rgb_success: bool,
    ir_success: bool,
) -> bool {
    match (run_rgb, run_ir) {
        (true, true) => match policy {
            "or" => rgb_success || ir_success,
            "and" => rgb_success && ir_success,
            // Fallback policy: both spectra must pass unless RGB ran and was too dark to judge.
            _ => {
                if !rgb_attempted {
                    rgb_success && ir_success
                } else if matches!(rgb_status, CaptureStatus::TooDark) {
                    ir_success
                } else {
                    rgb_success && ir_success
                }
            }
        },
        (true, false) => rgb_success,
        (false, true) => ir_success,
        (false, false) => false,
    }
}

pub(super) fn auth_streams(
    rgb_device: &str,
    ir_device: &str,
    has_rgb_templates: bool,
    has_ir_templates: bool,
) -> (bool, bool) {
    (
        !rgb_device.is_empty() && has_rgb_templates,
        !ir_device.is_empty() && has_ir_templates,
    )
}

pub(super) fn and_policy_unsatisfiable(
    policy: &str,
    rgb_device: &str,
    ir_device: &str,
    run_rgb: bool,
    run_ir: bool,
) -> bool {
    policy == "and" && !rgb_device.is_empty() && !ir_device.is_empty() && !(run_rgb && run_ir)
}

pub(super) fn process_frame_sync(
    checker: &mut FaceChecker,
    recognizer: &mut FaceRecognizer,
    frame: &Mat,
    keep_liveness_frame: bool,
) -> anyhow::Result<(CaptureStatus, Option<FaceData>)> {
    let (status, result_opt) = checker.capture_status(frame)?;

    if status != CaptureStatus::Usable {
        return Ok((status, None));
    }

    if let Some(res) = result_opt {
        let Some(kpss) = res.kpss else {
            return Ok((status, None));
        };
        let Some(mat_rgb) = res.mat_rgb else {
            return Ok((status, None));
        };

        let aligned = align_face(&mat_rgb, &kpss, 0)?;
        let embedding = recognizer.get_embedding(&aligned)?;

        let Some((x1, y1, x2, y2)) = res.bbox else {
            return Ok((status, None));
        };
        let liveness_frame = if keep_liveness_frame {
            Some(mat_rgb)
        } else {
            None
        };
        Ok((
            status,
            Some(FaceData {
                embedding,
                liveness_frame,
                frame_size: (res.width, res.height),
                bbox: [x1, y1, x2, y2],
                kpss,
                yaw: res.yaw,
                pitch: res.pitch,
            }),
        ))
    } else {
        Ok((status, None))
    }
}

// Strip the square padding first, since its black bars read as a replay bezel
// to the anti-spoof model.
pub(super) fn crop_liveness_face(data: &FaceData) -> anyhow::Result<image::RgbImage> {
    let mat_rgb = data
        .liveness_frame
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("liveness frame was not retained"))?;
    let rgb = mat_to_rgb(mat_rgb)?;
    let (frame_w, frame_h) = data.frame_size;
    let frame_w = frame_w.min(rgb.width()).max(1);
    let frame_h = frame_h.min(rgb.height()).max(1);
    let pad_x = (rgb.width() - frame_w) / 2;
    let pad_y = (rgb.height() - frame_h) / 2;
    let content = image::imageops::crop_imm(&rgb, pad_x, pad_y, frame_w, frame_h).to_image();
    let bbox = [
        data.bbox[0] - pad_x as f32,
        data.bbox[1] - pad_y as f32,
        data.bbox[2] - pad_x as f32,
        data.bbox[3] - pad_y as f32,
    ];
    crate::liveness::crop_face(&content, bbox)
}

pub fn load_eye_state(
    inference: &gaze_core::config::InferenceConfig,
) -> anyhow::Result<EyeStateClassifier> {
    let path = crate::models::ensure_eye_state_model(gaze_core::config::MODELS_DIR)?;
    EyeStateClassifier::new_with_inference(path.to_str().unwrap(), inference)
}

pub(super) fn eyes_closed_in_frame(
    eye_state: &Mutex<Option<EyeStateClassifier>>,
    data: &FaceData,
    threshold: f32,
) -> anyhow::Result<bool> {
    let frame = data
        .liveness_frame
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("frame was not retained for the eye-state check"))?;
    let eyes =
        eyes_from_kpss(&data.kpss).ok_or_else(|| anyhow::anyhow!("face landmarks are missing"))?;
    let rgb = mat_to_rgb(frame)?;
    let mut guard = eye_state.blocking_lock();
    let classifier = guard.as_mut().ok_or_else(|| {
        anyhow::anyhow!("duress detection is enabled but the eye-state model is unavailable")
    })?;
    let probabilities = classifier.closed_probabilities(&rgb, [eyes[0], eyes[1]])?;
    tracing::debug!(?probabilities, threshold, "Duress eye state");
    Ok(crate::duress::any_eye_closed(probabilities, threshold))
}

/// One row per enrolled face, holding (name, rgb_sim, rgb_pct, rgb_passed, ir_sim, ir_pct,
/// ir_passed) and sorted best match first.
pub(super) fn build_hybrid_scores(
    db: &UserDatabase,
    username: &str,
    rgb_threshold: f32,
    ir_threshold: f32,
    rgb_embed: Option<&ndarray::Array1<f32>>,
    ir_embed: Option<&ndarray::Array1<f32>>,
) -> Vec<(String, f64, f64, bool, f64, f64, bool)> {
    let rgb_scores = rgb_embed.and_then(|embed| {
        db.match_faces(username, embed, rgb_threshold, Spectrum::Rgb)
            .ok()
    });
    let ir_scores = ir_embed.and_then(|embed| {
        db.match_faces(username, embed, ir_threshold, Spectrum::Ir)
            .ok()
    });

    let mut final_scores = Vec::new();
    if let Ok(faces) = db.list_faces(username) {
        for (name, _, _, _) in faces {
            let (rgb_sim, rgb_pct, rgb_passed) = if let Some(ref scores) = rgb_scores {
                if let Some(score) = scores.iter().find(|s| s.0 == name) {
                    (score.1 as f64, score.2 as f64, score.3)
                } else {
                    (0.0, 0.0, false)
                }
            } else {
                (0.0, 0.0, false)
            };

            let (ir_sim, ir_pct, ir_passed) = if let Some(ref scores) = ir_scores {
                if let Some(score) = scores.iter().find(|s| s.0 == name) {
                    (score.1 as f64, score.2 as f64, score.3)
                } else {
                    (0.0, 0.0, false)
                }
            } else {
                (0.0, 0.0, false)
            };

            final_scores.push((
                name, rgb_sim, rgb_pct, rgb_passed, ir_sim, ir_pct, ir_passed,
            ));
        }
    }

    final_scores.sort_by(|a, b| {
        let a_max = a.1.max(a.4);
        let b_max = b.1.max(b.4);
        b_max
            .partial_cmp(&a_max)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    final_scores
}

impl AuthDaemon {
    pub(super) async fn start_verification(
        &self,
        ctxt: SignalEmitter<'_>,
        header: Header<'_>,
        pam_service: Option<String>,
        require_keyring: bool,
    ) -> fdo::Result<()> {
        let claim = self.check_claim(&header).await?;
        self.ensure_auth_not_aborted(&header).await?;

        let resumed = self.resume_pending.load(Ordering::SeqCst);
        let resume_pending = self.resume_pending.clone();

        let username = claim.username.clone();
        let signal_destination = Self::signal_destination(&claim.sender)?;
        self.cancel_active_tasks().await;

        let (tx, mut rx) = oneshot::channel();
        *self.active_cancel.lock().await = Some(tx);

        let detector_arc = self.detector.clone();
        let recognizer_rgb_arc = self.recognizer_rgb.clone();
        let recognizer_ir_arc = self.recognizer_ir.clone();
        let liveness_arc = self.liveness.clone();
        let db_arc = self.db.clone();
        let rgb_threshold_arc = self.rgb_threshold.clone();
        let ir_threshold_arc = self.ir_threshold.clone();

        let config = self.current_config().await;
        let active_session = active_session().await;
        let surface = Self::classify_surface(pam_service.as_deref(), active_session.as_ref());
        let lock_elapsed_ms = self.lock_elapsed_ms(active_session.as_ref()).await;
        let delay = Duration::from_millis(config.auth.start_delay_after_lock_ms(
            resumed,
            surface,
            lock_elapsed_ms,
        ));
        info!(
            service = pam_service.as_deref().unwrap_or("<unknown>"),
            ?surface,
            lock_elapsed_ms,
            "Face auth requested"
        );
        let abort_if_lid_closed = *self.abort_if_lid_closed.lock().await;
        let rgb_device = self.rgb_device.lock().await.clone();
        let ir_device = self.ir_device.lock().await.clone();
        let emitter_enabled = *self.emitter_enabled.lock().await;
        let mut ir_node = self.ir_node.lock().await.clone();
        if emitter_enabled
            && ir_node.is_empty()
            && let Some(resolved) = gaze_vision::camera::resolve_node(&ir_device)
        {
            *self.ir_node.lock().await = resolved.clone();
            ir_node = resolved;
        }
        let liveness_cfg = self.liveness_config.lock().await.clone();
        // Check the state used by this attempt, not the Config property (which reads disk).
        // This exact liveness snapshot is moved into the verification task below.
        validate_keyring_verification(
            require_keyring,
            &liveness_cfg,
            self.db.lock().await.is_encrypted(),
        )?;
        let hybrid_policy = self.hybrid_policy.lock().await.clone();
        let serial_capture = *self.serial_capture.lock().await;
        let duress_cfg = config.duress.clone();
        if duress_cfg.enabled && self.eye_state.lock().await.is_none() {
            match load_eye_state(&config.inference) {
                Ok(classifier) => *self.eye_state.lock().await = Some(classifier),
                Err(e) => {
                    warn!("Duress detection is enabled but the eye-state model failed to load: {e}")
                }
            }
        }
        let eye_state_arc = self.eye_state.clone();
        let duress_lockout = self.duress_lockout.clone();
        let duress_locked = duress_lockout.is_locked(&username);
        let conn = ctxt.connection().clone();
        let path = ctxt.path().to_owned();

        self.rt_handle.spawn(async move {
            let ctxt = match SignalEmitter::new(&conn, path) {
                Ok(emitter) => emitter.set_destination(signal_destination),
                Err(e) => {
                    error!("Failed to create signal emitter: {e}");
                    return;
                }
            };

            if duress_locked {
                info!(
                    "Face authentication for {} is locked after a duress signal until a password login",
                    username
                );
                let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), CaptureStatus::Unused, CaptureStatus::Unused).await;
                return;
            }

            let db = db_arc.lock().await;
            let faces_list = db.list_faces(&username).unwrap_or_default();
            let mut has_rgb_templates = false;
            let mut has_ir_templates = false;
            for (_, _, has_rgb, has_ir) in &faces_list {
                if *has_rgb {
                    has_rgb_templates = true;
                }
                if *has_ir {
                    has_ir_templates = true;
                }
            }
            drop(db);

            let (run_rgb, run_ir) = auth_streams(
                &rgb_device,
                &ir_device,
                has_rgb_templates,
                has_ir_templates,
            );

            if !run_rgb && !run_ir {
                error!("No matching templates or cameras configured for auth");
                let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), CaptureStatus::NoFace, CaptureStatus::NoFace).await;
                return;
            }

            if and_policy_unsatisfiable(&hybrid_policy, &rgb_device, &ir_device, run_rgb, run_ir) {
                error!(
                    run_rgb,
                    run_ir,
                    has_rgb_templates,
                    has_ir_templates,
                    "Hybrid policy \"and\" requires both spectra but {} has no {} templates; \
                     refusing to authenticate on one spectrum. Re-enrol to cover both.",
                    username,
                    if has_rgb_templates { "IR" } else { "RGB" }
                );
                let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), CaptureStatus::NoFace, CaptureStatus::NoFace).await;
                return;
            }

            if !delay.is_zero() {
                info!(?delay, resumed, ?surface, "Delaying face auth before capture");
                if tokio::time::timeout(delay, &mut rx).await.is_ok() {
                    info!("VerifyStart: cancelled during start delay");
                    let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), CaptureStatus::NoFace, CaptureStatus::NoFace).await;
                    return;
                }
                if abort_if_lid_closed && Self::is_lid_closed().await {
                    warn!("Laptop lid is closed, aborting face auth");
                    let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), CaptureStatus::NoFace, CaptureStatus::NoFace).await;
                    return;
                }
            }

            resume_pending.store(false, Ordering::SeqCst);

            info!(
                liveness_enabled = liveness_cfg.enabled,
                liveness_threshold = liveness_cfg.effective_threshold(),
                run_rgb = run_rgb,
                run_ir = run_ir,
                serial_capture = serial_capture,
                "VerifyStart: sensing faces for user {}",
                username
            );

            let (result_tx, mut result_rx) = tokio::sync::mpsc::channel::<VerifyMsg>(10);
            let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            // Signals that the RGB phase released its camera so the IR thread can take it,
            // letting single-function UVC devices (e.g. Logitech Brio) run hybrid verify.
            let rgb_phase_done = Arc::new(std::sync::atomic::AtomicBool::new(false));

            let mut rgb_thread = None;
            if run_rgb {
                let stop_clone = stop_flag.clone();
                let tx = result_tx.clone();
                let detector_arc = detector_arc.clone();
                let config_clone = config.clone();
                let recognizer_rgb_arc = recognizer_rgb_arc.clone();
                let liveness_arc = liveness_arc.clone();
                let db_arc = db_arc.clone();
                let username_clone = username.clone();
                let rgb_threshold_arc = rgb_threshold_arc.clone();
                let liveness_enabled = liveness_cfg.enabled;
                let liveness_threshold = liveness_cfg.effective_threshold();
                let rgb_device_clone = rgb_device.clone();
                let rgb_phase_done_clone = rgb_phase_done.clone();
                let hybrid_policy_clone = hybrid_policy.clone();
                let eye_state_arc = eye_state_arc.clone();
                let duress_enabled = duress_cfg.enabled;
                let duress_threshold = duress_cfg.effective_closed_threshold();
                let duress_hold = duress_cfg.effective_hold();

                rgb_thread = Some(std::thread::spawn(move || {
                    // Set on every exit path (incl. panic) once the RGB camera is released.
                    // Declared before `cam` so `cam` drops first and release precedes the signal.
                    struct RgbPhaseGuard(Arc<std::sync::atomic::AtomicBool>);
                    impl Drop for RgbPhaseGuard {
                        fn drop(&mut self) {
                            self.0.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    let _rgb_phase_guard = RgbPhaseGuard(rgb_phase_done_clone);
                    // In serial mode (IR also runs) yield the camera after a budget even
                    // without a match, so the IR spectrum can still be captured.
                    let rgb_deadline = rgb_yields_camera_on_budget(run_ir, serial_capture)
                        .then(|| Instant::now() + VERIFY_SERIAL_RGB_BUDGET);
                    let mut yielded_to_ir = false;

                    let mut cam = match Camera::open_privileged(&rgb_device_clone) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(VerifyMsg::Error(format!("RGB Camera open error: {e}")));
                            return;
                        }
                    };
                    tracing::debug!("RGB camera opened successfully at: {}", rgb_device_clone);

                    let mut checker = FaceChecker::new(detector_arc, &config_clone, Spectrum::Rgb, false);
                    let dark_hands_over_to_ir = should_yield_rgb_to_ir(
                        &hybrid_policy_clone,
                        run_ir,
                        CaptureStatus::TooDark,
                    );
                    let mut warmup = RgbWarmupGate::new(config_clone.cameras.dark_luma_threshold);
                    let mut logged_dark_stream = false;
                    let mut logged_rgb_luma_statuses = Vec::new();
                    let mut live_scores: Vec<f32> = Vec::new();
                    let mut landmark_seq: Vec<[(f32, f32); 5]> = Vec::new();
                    let mut duress = DuressTracker::new(duress_hold);

                    while let Some(frame) = cam.next_interruptible(&stop_clone) {
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        if let Some(deadline) = rgb_deadline
                            && Instant::now() >= deadline
                        {
                            // Serial mode hands the camera to the IR phase even without a
                            // match, so hybrid auth can still capture the IR spectrum.
                            yielded_to_ir = true;
                            break;
                        }

                        if !dark_hands_over_to_ir {
                            match warmup.classify_with_luma(&frame) {
                                (RgbFrameKind::Lit, _) => {}
                                (RgbFrameKind::WarmupDark, _) => continue,
                                (RgbFrameKind::SettledDark, luma) => {
                                    if !logged_dark_stream {
                                        let message = format!(
                                            "RGB stream remains dark after warmup: mean_luma={luma}"
                                        );
                                        info!("{message}");
                                        let _ = tx.blocking_send(VerifyMsg::Diagnostic(message));
                                        logged_dark_stream = true;
                                    }
                                }
                                (RgbFrameKind::SteadyDark, luma) => {
                                    if !logged_dark_stream {
                                        let message = format!(
                                            "RGB stream never brightened: mean_luma={luma}"
                                        );
                                        info!("{message}");
                                        let _ = tx.blocking_send(VerifyMsg::Diagnostic(message));
                                        logged_dark_stream = true;
                                    }
                                }
                            }
                        }

                        let (status, embed_opt) = {
                            let mut recognizer = recognizer_rgb_arc.blocking_lock();
                            match process_frame_sync(&mut checker, &mut recognizer, &frame, liveness_enabled || duress_enabled) {
                                Ok(res) => res,
                                Err(_) => (CaptureStatus::NoFace, None),
                            }
                        };
                        tracing::debug!("Processed RGB frame: status={:?}, embedding_extracted={}", status, embed_opt.is_some());

                        if let Some(luma) = checker.rgb_face_luma()
                            && !logged_rgb_luma_statuses.contains(&status)
                        {
                            let message = format!(
                                "RGB face region: mean_luma={}, rolling_mean_luma={:.1}, threshold={}, status={status:?}",
                                luma.mean, luma.rolling_mean, luma.threshold
                            );
                            info!("{message}");
                            let _ = tx.blocking_send(VerifyMsg::Diagnostic(message));
                            logged_rgb_luma_statuses.push(status);
                        }

                        let latest_embed = embed_opt.as_ref().map(|d| d.embedding.clone());
                        let _ = tx.try_send(VerifyMsg::Status(Spectrum::Rgb, status, latest_embed, cam.fps()));

                        if should_yield_rgb_to_ir(&hybrid_policy_clone, run_ir, status) {
                            yielded_to_ir = true;
                            break;
                        }

                        if status == CaptureStatus::Usable && let Some(data) = embed_opt {
                            let threshold = *rgb_threshold_arc.blocking_lock();
                            let db = db_arc.blocking_lock();
                            let scores = match db.match_faces(&username_clone, &data.embedding, threshold, Spectrum::Rgb) {
                                Ok(s) => s,
                                Err(e) => {
                                    let _ = tx.blocking_send(VerifyMsg::Error(format!("DB error: {e}")));
                                    return;
                                }
                            };
                            drop(db);

                            tracing::debug!("RGB match scores: {:?}", scores);

                            let matched = scores.iter().any(|(_, _, _, passed, _)| *passed);
                            if matched {
                                if duress_enabled {
                                    let closed = match eyes_closed_in_frame(&eye_state_arc, &data, duress_threshold) {
                                        Ok(closed) => closed,
                                        Err(e) => {
                                            let _ = tx.blocking_send(VerifyMsg::Error(format!("Duress check failed: {e}")));
                                            return;
                                        }
                                    };
                                    if duress.observe(closed, Instant::now()) {
                                        let _ = tx.blocking_send(VerifyMsg::Duress(Spectrum::Rgb));
                                        return;
                                    }
                                    if closed {
                                        continue;
                                    }
                                }

                                let mut liveness_passed = true;
                                if liveness_enabled {
                                    if let Some(eyes) = eyes_from_kpss(&data.kpss) {
                                        landmark_seq.push(eyes);
                                    }
                                    let liveness_face = match crop_liveness_face(&data) {
                                        Ok(face) => face,
                                        Err(e) => {
                                            error!("Liveness crop failed: {e}");
                                            continue;
                                        }
                                    };
                                    let mut live_guard = liveness_arc.blocking_lock();
                                    let Some(detector) = live_guard.as_mut() else {
                                        let _ = tx.blocking_send(VerifyMsg::Error(
                                            "Liveness is enabled but the anti-spoof model is unavailable".to_string(),
                                        ));
                                        return;
                                    };
                                    let live_score = match detector.live_score(&liveness_face) {
                                        Ok(score) => score,
                                        Err(e) => {
                                            let _ = tx.blocking_send(VerifyMsg::Error(format!(
                                                "Liveness inference failed: {e}"
                                            )));
                                            return;
                                        }
                                    };
                                    drop(live_guard);
                                    live_scores.push(live_score);

                                    let model_pass = crate::liveness::liveness_passes(&live_scores, liveness_threshold as f32);
                                    let motion = crate::liveness::eye_motion_is_live(&landmark_seq, None);
                                    let confirmed_static = crate::liveness::confirmed_static(&motion);
                                    liveness_passed = model_pass && !confirmed_static;

                                    tracing::debug!(
                                        "Liveness checked: score={:?}, pass={}, motion={:?}, confirmed_static={}, overall={}",
                                        live_scores,
                                        model_pass,
                                        motion,
                                        confirmed_static,
                                        liveness_passed
                                    );
                                }

                                if liveness_passed {
                                    let _ = tx.blocking_send(VerifyMsg::Success(Spectrum::Rgb, data.embedding));
                                    return;
                                }
                            }
                        }
                    }

                    if !yielded_to_ir && !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                        // A device taken by another program only fails once it tries to stream,
                        // so this is where "already in use" surfaces.
                        let reason = cam.take_stream_error().unwrap_or_else(|| {
                            "RGB camera stream stopped unexpectedly".to_string()
                        });
                        let _ = tx.blocking_send(VerifyMsg::Error(reason));
                    }
                }));
            }

            let mut ir_thread = None;
            if run_ir {
                let stop_clone = stop_flag.clone();
                let tx = result_tx.clone();
                let detector_arc = detector_arc.clone();
                let config_clone = config.clone();
                let recognizer_ir_arc = recognizer_ir_arc.clone();
                let db_arc = db_arc.clone();
                let username_clone = username.clone();
                let ir_threshold_arc = ir_threshold_arc.clone();
                let liveness_enabled = liveness_cfg.enabled;
                let ir_device_clone = ir_device.clone();
                let ir_node_clone = ir_node.clone();
                let emitter_enabled = emitter_enabled;
                let serial_capture = serial_capture;
                let rgb_phase_done_clone = rgb_phase_done.clone();
                let eye_state_arc = eye_state_arc.clone();
                let duress_enabled = duress_cfg.enabled;
                let duress_threshold = duress_cfg.effective_closed_threshold();
                let duress_hold = duress_cfg.effective_hold();

                ir_thread = Some(std::thread::spawn(move || {
                    // Wait for RGB to release its camera before opening IR and firing the emitter,
                    // so single-function devices keep one live stream. Bail if verify passed.
                    if ir_waits_for_rgb(run_rgb, serial_capture) {
                        while !rgb_phase_done_clone.load(std::sync::atomic::Ordering::Relaxed)
                            && !stop_clone.load(std::sync::atomic::Ordering::Relaxed)
                        {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            return;
                        }
                    }

                    let mut emitter = EmitterGuard::engage(
                        &CameraKind::Ir { source: ir_device_clone.clone(), node: ir_node_clone.clone() },
                        emitter_enabled
                    );
                    if let Some(message) = emitter.activation_message() {
                        let _ = tx.blocking_send(VerifyMsg::Diagnostic(message.to_owned()));
                    }

                    let mut cam = match Camera::open_ir_privileged(&ir_device_clone) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(VerifyMsg::Error(format!("IR Camera open error: {e}")));
                            return;
                        }
                    };
                    tracing::debug!("IR camera opened successfully at: {}", ir_device_clone);

                    let _ = tx.blocking_send(VerifyMsg::PhaseStarted(Spectrum::Ir));

                    let mut checker = FaceChecker::new(detector_arc, &config_clone, Spectrum::Ir, false);
                    let mut dark_gate = IrDarkFrameGate::new(config_clone.cameras.dark_luma_threshold);
                    let mut logged_lit_luma = false;
                    let mut logged_dark_luma = false;
                    let mut landmark_seq: Vec<[(f32, f32); 5]> = Vec::new();
                    let mut duress = DuressTracker::new(duress_hold);

                    while let Some(frame) = cam.next_interruptible(&stop_clone) {
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        emitter.stream_started();

                        let (frame_kind, luma) = dark_gate.classify_with_luma(&frame);
                        match frame_kind {
                            IrFrameKind::Lit => {
                                if !logged_lit_luma {
                                    let message = format!(
                                        "IR stream produced a lit frame: mean_luma={luma}"
                                    );
                                    info!("{message}");
                                    let _ = tx.blocking_send(VerifyMsg::Diagnostic(message));
                                    logged_lit_luma = true;
                                }
                            }
                            // A gap between emitter strobes, not a fault, so drop it silently.
                            IrFrameKind::StrobeDark => continue,
                            IrFrameKind::EmitterDark => {
                                if !logged_dark_luma {
                                    let message = format!(
                                        "IR stream remains dark after emitter warmup: mean_luma={luma}"
                                    );
                                    info!("{message}");
                                    let _ = tx.blocking_send(VerifyMsg::Diagnostic(message));
                                    logged_dark_luma = true;
                                }
                                let _ = tx.try_send(VerifyMsg::Status(Spectrum::Ir, CaptureStatus::TooDark, None, cam.fps()));
                                continue;
                            }
                        }

                        let (status, embed_opt) = {
                            let mut recognizer = recognizer_ir_arc.blocking_lock();
                            match process_frame_sync(&mut checker, &mut recognizer, &frame, duress_enabled) {
                                Ok(res) => res,
                                Err(_) => (CaptureStatus::NoFace, None),
                            }
                        };
                        tracing::debug!("Processed IR frame: status={:?}, embedding_extracted={}", status, embed_opt.is_some());

                        let latest_embed = embed_opt.as_ref().map(|d| d.embedding.clone());
                        let _ = tx.try_send(VerifyMsg::Status(Spectrum::Ir, status, latest_embed, cam.fps()));

                        if status == CaptureStatus::Usable && let Some(data) = embed_opt {
                            let threshold = *ir_threshold_arc.blocking_lock();
                            let db = db_arc.blocking_lock();
                            let scores = match db.match_faces(&username_clone, &data.embedding, threshold, Spectrum::Ir) {
                                Ok(s) => s,
                                Err(e) => {
                                    let _ = tx.blocking_send(VerifyMsg::Error(format!("DB error: {e}")));
                                    return;
                                }
                            };
                            drop(db);

                            tracing::debug!("IR match scores: {:?}", scores);

                            let matched = scores.iter().any(|(_, _, _, passed, _)| *passed);
                            if matched {
                                if duress_enabled {
                                    let closed = match eyes_closed_in_frame(&eye_state_arc, &data, duress_threshold) {
                                        Ok(closed) => closed,
                                        Err(e) => {
                                            let _ = tx.blocking_send(VerifyMsg::Error(format!("Duress check failed: {e}")));
                                            return;
                                        }
                                    };
                                    if duress.observe(closed, Instant::now()) {
                                        let _ = tx.blocking_send(VerifyMsg::Duress(Spectrum::Ir));
                                        return;
                                    }
                                    if closed {
                                        continue;
                                    }
                                }

                                let mut liveness_passed = true;
                                if liveness_enabled {
                                    if let Some(eyes) = eyes_from_kpss(&data.kpss) {
                                        landmark_seq.push(eyes);
                                    }
                                    let motion = crate::liveness::eye_motion_is_live(&landmark_seq, None);
                                    liveness_passed = crate::liveness::motion_confirms_live(
                                        &motion,
                                        crate::liveness::MIN_MOTION_PAIRS,
                                    );

                                    tracing::debug!(
                                        "Liveness checked (IR): motion={:?}, overall={}",
                                        motion,
                                        liveness_passed
                                    );
                                }

                                if liveness_passed {
                                    let _ = tx.blocking_send(VerifyMsg::Success(Spectrum::Ir, data.embedding));
                                    return;
                                }
                            }
                        }
                    }

                    if !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                        let reason = cam.take_stream_error().unwrap_or_else(|| {
                            "IR camera stream stopped unexpectedly".to_string()
                        });
                        let _ = tx.blocking_send(VerifyMsg::Error(reason));
                    }
                }));
            }

            drop(result_tx);

            let mut last_emitted_status: Option<CaptureStatus> = None;
            let mut rgb_status = CaptureStatus::Unused;
            let mut ir_status = CaptureStatus::Unused;
            let mut rgb_attempted = false;
            let mut dark_since: Option<Instant> = None;
            let mut last_face_at = Instant::now();
            let mut last_usable_at = Instant::now();
            let mut frames_seen: u32 = 0;

            let mut rgb_success_embed = None;
            let mut ir_success_embed = None;
            let mut rgb_latest_embed = None;
            let mut ir_latest_embed = None;

            macro_rules! hybrid_scores {
                () => {{
                    let rgb_threshold = *rgb_threshold_arc.lock().await;
                    let ir_threshold = *ir_threshold_arc.lock().await;
                    let db = db_arc.lock().await;
                    let final_scores = build_hybrid_scores(
                        &db,
                        &username,
                        rgb_threshold,
                        ir_threshold,
                        rgb_success_embed.as_ref().or(rgb_latest_embed.as_ref()),
                        ir_success_embed.as_ref().or(ir_latest_embed.as_ref()),
                    );
                    drop(db);
                    final_scores
                }};
            }

            macro_rules! emit_verify_with_scores {
                ($result:expr) => {{
                    let final_scores = hybrid_scores!();
                    let _ = Self::verify_status(&ctxt, $result, final_scores, rgb_status, ir_status).await;
                }};
            }

            macro_rules! finish_if_auth_passed {
                () => {{
                    if hybrid_auth_passed(
                        &hybrid_policy,
                        run_rgb,
                        run_ir,
                        rgb_attempted,
                        rgb_status,
                        rgb_success_embed.is_some(),
                        ir_success_embed.is_some(),
                    ) {
                        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        emit_verify_with_scores!(VerifyResult::VerifyMatch);
                        true
                    } else {
                        false
                    }
                }};
            }

            loop {
                tokio::select! {
                    _ = &mut rx => {
                        info!("VerifyStart: cancelled");
                        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        // Report the camera as idle, not as a rejection: a cancelled attempt
                        // never decided anything, and a rejection counts toward lockout.
                        let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), CaptureStatus::Unused, CaptureStatus::Unused).await;
                        break;
                    }
                    _ = tokio::time::sleep(VERIFY_WATCHDOG_POLL) => {
                        if let Some(give_up) = verify_give_up(last_face_at.elapsed(), last_usable_at.elapsed()) {
                            info!("VerifyStart: {}", give_up.reason());
                            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                            let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), rgb_status, ir_status).await;
                            break;
                        }
                    }
                    msg_opt = result_rx.recv() => {
                        let Some(msg) = msg_opt else {
                            warn!("VerifyStart: all capture threads exited without a result");
                            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                            let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), rgb_status, ir_status).await;
                            break;
                        };
                        match msg {
                            VerifyMsg::PhaseStarted(Spectrum::Ir) if serial_capture => {
                                // RGB and IR run serially on single-function cameras. Give
                                // IR a fresh no-face window after RGB releases the device.
                                last_face_at = Instant::now();
                                last_usable_at = Instant::now();
                                frames_seen = 0;
                                dark_since = None;
                            }
                            VerifyMsg::PhaseStarted(Spectrum::Rgb) => {
                                last_face_at = Instant::now();
                                last_usable_at = Instant::now();
                                frames_seen = 0;
                                dark_since = None;
                            }
                            VerifyMsg::PhaseStarted(_) => {}
                            VerifyMsg::Diagnostic(message) => {
                                let _ = Self::verify_diagnostic(&ctxt, &message).await;
                            }
                            VerifyMsg::Status(spectrum, status, embed_opt, fps) => {
                                let has_face = embed_opt.is_some();
                                match spectrum {
                                    Spectrum::Rgb => {
                                        rgb_status = status;
                                        rgb_attempted = true;
                                        if let Some(embed) = embed_opt {
                                            rgb_latest_embed = Some(embed);
                                        }
                                    }
                                    Spectrum::Ir => {
                                        ir_status = status;
                                        if let Some(embed) = embed_opt {
                                            ir_latest_embed = Some(embed);
                                        }
                                    }
                                }

                                if status.indicates_face() {
                                    last_face_at = Instant::now();
                                }

                                if has_face {
                                    last_usable_at = Instant::now();
                                    frames_seen += 1;
                                    if frames_seen > liveness_cfg.effective_max_frames(fps) {
                                        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                        let final_scores = hybrid_scores!();
                                        let matched = final_scores
                                            .iter()
                                            .any(|face| face.3 || face.6);
                                        info!(matched, "VerifyStart: frame budget spent");
                                        if matched {
                                            let _ = Self::verify_diagnostic(&ctxt, LIVENESS_GATE_DIAGNOSTIC).await;
                                        }
                                        let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, final_scores, rgb_status, ir_status).await;
                                        break;
                                    }
                                }

                                Self::emit_effective_face_status(
                                    &ctxt,
                                    &mut last_emitted_status,
                                    rgb_status,
                                    ir_status,
                                ).await;

                                let both_dark = match (run_rgb, run_ir) {
                                    (true, true) => rgb_status == CaptureStatus::TooDark && ir_status == CaptureStatus::TooDark,
                                    (true, false) => rgb_status == CaptureStatus::TooDark,
                                    (false, true) => ir_status == CaptureStatus::TooDark,
                                    (false, false) => false,
                                };

                                if both_dark {
                                    let started = *dark_since.get_or_insert_with(Instant::now);
                                    if started.elapsed() >= VERIFY_TOO_DARK_TIMEOUT {
                                        info!(
                                            "VerifyStart: giving up after {}ms of dark frames",
                                            VERIFY_TOO_DARK_TIMEOUT.as_millis()
                                        );
                                        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                        let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), rgb_status, ir_status).await;
                                        break;
                                    }
                                } else {
                                    dark_since = None;
                                }

                                if let Some(give_up) = verify_give_up(last_face_at.elapsed(), last_usable_at.elapsed()) {
                                    info!("VerifyStart: {}", give_up.reason());
                                    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                    let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), rgb_status, ir_status).await;
                                    break;
                                }

                                if finish_if_auth_passed!() {
                                    break;
                                }
                            }
                            VerifyMsg::Success(spectrum, embedding) => {
                                match spectrum {
                                    Spectrum::Rgb => {
                                        rgb_success_embed = Some(embedding);
                                        rgb_attempted = true;
                                    }
                                    Spectrum::Ir => ir_success_embed = Some(embedding),
                                }

                                if finish_if_auth_passed!() {
                                    break;
                                }
                            }
                            VerifyMsg::Duress(spectrum) => {
                                stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                info!(?spectrum, "Duress signal held; locking face authentication for {} until a password login", username);
                                if let Err(e) = duress_lockout.lock(&username) {
                                    error!("Failed to persist the duress lockout, holding it in memory: {e}");
                                }
                                let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), rgb_status, ir_status).await;
                                break;
                            }
                            VerifyMsg::Error(e) => {
                                error!("VerifyStart loop error: {e}");
                                stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                // The verdict alone reads as a face that was not found.
                                let _ = Self::verify_diagnostic(&ctxt, &e).await;
                                // Idle, not rejected: the run broke off instead of deciding, and
                                // a hardware failure must not count against the lockout budget.
                                let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), CaptureStatus::Unused, CaptureStatus::Unused).await;
                                break;
                            }
                        }
                    }
                }
            }

            if let Some(t) = rgb_thread {
                let _ = t.join();
            }
            if let Some(t) = ir_thread {
                let _ = t.join();
            }
        });

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaze_core::dbus::CaptureStatus;

    #[test]
    fn watchdog_polls_faster_than_the_timeouts_it_guards() {
        use super::{
            VERIFY_NO_FACE_TIMEOUT, VERIFY_NO_USABLE_TIMEOUT, VERIFY_TOO_DARK_TIMEOUT,
            VERIFY_WATCHDOG_POLL,
        };

        assert!(VERIFY_WATCHDOG_POLL < VERIFY_NO_FACE_TIMEOUT);
        assert!(VERIFY_WATCHDOG_POLL < VERIFY_TOO_DARK_TIMEOUT);
        assert!(VERIFY_WATCHDOG_POLL < VERIFY_NO_USABLE_TIMEOUT);
        assert!(!VERIFY_WATCHDOG_POLL.is_zero());
    }

    #[test]
    fn a_warming_camera_still_reports_dark_before_the_no_face_deadline() {
        use super::{RgbWarmupGate, VERIFY_NO_FACE_TIMEOUT, VERIFY_TOO_DARK_TIMEOUT};

        assert!(RgbWarmupGate::WARMUP + VERIFY_TOO_DARK_TIMEOUT < VERIFY_NO_FACE_TIMEOUT);
    }

    // A backstop that fired first would report a timeout for a run the daemon had already decided.
    #[test]
    fn every_daemon_deadline_lands_inside_the_client_backstop() {
        use super::{VERIFY_NO_FACE_TIMEOUT, VERIFY_NO_USABLE_TIMEOUT, VERIFY_TOO_DARK_TIMEOUT};

        let backstop = gaze_core::dbus::VERIFY_CLIENT_TIMEOUT;
        for deadline in [
            VERIFY_NO_FACE_TIMEOUT,
            VERIFY_NO_USABLE_TIMEOUT,
            VERIFY_TOO_DARK_TIMEOUT,
        ] {
            assert!(
                deadline < backstop,
                "{deadline:?} must fire before {backstop:?}"
            );
        }
    }

    // Before the usable deadline existed this case ran forever.
    #[test]
    fn a_face_that_never_becomes_usable_still_hits_a_deadline() {
        use super::{VERIFY_NO_USABLE_TIMEOUT, VerifyGiveUp, verify_give_up};

        let never_stale = std::time::Duration::ZERO;
        assert_eq!(verify_give_up(never_stale, never_stale), None);
        assert_eq!(
            verify_give_up(never_stale, VERIFY_NO_USABLE_TIMEOUT),
            Some(VerifyGiveUp::NoUsableFrame)
        );
    }

    #[test]
    fn a_vanished_face_still_reports_the_no_face_deadline_first() {
        use super::{
            VERIFY_NO_FACE_TIMEOUT, VERIFY_NO_USABLE_TIMEOUT, VerifyGiveUp, verify_give_up,
        };

        assert_eq!(
            verify_give_up(VERIFY_NO_FACE_TIMEOUT, VERIFY_NO_USABLE_TIMEOUT),
            Some(VerifyGiveUp::NoFace)
        );
        assert_eq!(
            verify_give_up(VERIFY_NO_FACE_TIMEOUT, std::time::Duration::ZERO),
            Some(VerifyGiveUp::NoFace)
        );
    }

    // Every status that counts as a face but carries no embedding relies on the usable deadline.
    #[test]
    fn framing_hints_and_ready_count_as_a_face_without_being_usable() {
        for status in [
            CaptureStatus::Clipped,
            CaptureStatus::NotCentered,
            CaptureStatus::TooFar,
            CaptureStatus::TooClose,
            CaptureStatus::Ready,
        ] {
            assert!(
                status.indicates_face(),
                "{status:?} refreshes the no-face deadline"
            );
            assert_ne!(
                status,
                CaptureStatus::Usable,
                "{status:?} never reaches the embedding path in process_frame_sync"
            );
        }
    }

    #[test]
    fn a_stalled_capture_stream_still_reaches_the_no_face_deadline() {
        use super::VERIFY_WATCHDOG_POLL;

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let (_retained_tx, mut rx) = tokio::sync::mpsc::channel::<u32>(10);
            let deadline = std::time::Duration::from_millis(500);
            let started = std::time::Instant::now();
            let mut gave_up = false;

            loop {
                tokio::select! {
                    _ = tokio::time::sleep(VERIFY_WATCHDOG_POLL) => {
                        if started.elapsed() >= deadline {
                            gave_up = true;
                            break;
                        }
                    }
                    msg = rx.recv() => {
                        if msg.is_none() {
                            break;
                        }
                    }
                }
            }

            assert!(gave_up);
            assert!(started.elapsed() < deadline * 4);
        });
    }

    #[test]
    fn liveness_crop_excludes_square_padding_bars() {
        use super::{FaceData, crop_liveness_face};
        use opencv::core::{CV_8UC3, Mat, Scalar};

        let frame = Mat::new_rows_cols_with_default(480, 640, CV_8UC3, Scalar::all(255.0)).unwrap();
        let padded = gaze_vision::detect::FaceDetector::pad_to_square(&frame).unwrap();

        let data = FaceData {
            embedding: ndarray::Array1::zeros(512),
            liveness_frame: Some(padded),
            frame_size: (640, 480),
            // The 2.7x crop margin around this bbox reaches both padding bars.
            bbox: [220.0, 200.0, 420.0, 440.0],
            kpss: ndarray::Array3::zeros((1, 5, 2)),
            yaw: 0.0,
            pitch: 0.0,
        };

        let crop = crop_liveness_face(&data).unwrap();
        assert!(
            crop.pixels().all(|p| p.0 == [255, 255, 255]),
            "liveness crop must not contain padding pixels"
        );
    }

    #[test]
    fn authentication_starts_only_streams_with_a_camera_and_matching_templates() {
        assert_eq!(
            auth_streams("primary", "/dev/video2", true, true),
            (true, true)
        );
        assert_eq!(
            auth_streams("primary", "/dev/video2", true, false),
            (true, false)
        );
        assert_eq!(
            auth_streams("primary", "/dev/video2", false, true),
            (false, true)
        );
        assert_eq!(auth_streams("", "/dev/video2", true, true), (false, true));
        assert_eq!(auth_streams("primary", "", true, true), (true, false));
        assert_eq!(auth_streams("", "", true, true), (false, false));
    }

    #[test]
    fn independent_camera_functions_capture_both_spectra_at_once() {
        assert!(!ir_waits_for_rgb(true, false));
        assert!(!rgb_yields_camera_on_budget(true, false));
    }

    #[test]
    fn a_shared_camera_function_still_serializes_the_two_phases() {
        assert!(ir_waits_for_rgb(true, true));
        assert!(rgb_yields_camera_on_budget(true, true));
    }

    #[test]
    fn a_lone_spectrum_never_waits_on_the_other() {
        for serial_capture in [true, false] {
            assert!(!ir_waits_for_rgb(false, serial_capture), "{serial_capture}");
            assert!(
                !rgb_yields_camera_on_budget(false, serial_capture),
                "{serial_capture}"
            );
        }
    }

    #[test]
    fn parallel_capture_does_not_weaken_the_and_policy() {
        for (rgb_success, ir_success) in [(true, false), (false, true), (false, false)] {
            assert!(
                !hybrid_auth_passed(
                    "and",
                    true,
                    true,
                    true,
                    CaptureStatus::Usable,
                    rgb_success,
                    ir_success
                ),
                "{rgb_success} {ir_success}"
            );
        }
        assert!(hybrid_auth_passed(
            "and",
            true,
            true,
            true,
            CaptureStatus::Usable,
            true,
            true
        ));
    }

    #[test]
    fn parallel_capture_keeps_rgb_latching_on_dark_for_the_fallback_policy() {
        assert!(should_yield_rgb_to_ir(
            "fallback_on_dark",
            true,
            CaptureStatus::TooDark
        ));
        assert!(hybrid_auth_passed(
            "fallback_on_dark",
            true,
            true,
            true,
            CaptureStatus::TooDark,
            false,
            true
        ));
    }

    #[test]
    fn and_policy_refuses_to_degrade_to_one_spectrum() {
        let (run_rgb, run_ir) = auth_streams("primary", "/dev/video2", true, false);
        assert!(and_policy_unsatisfiable(
            "and",
            "primary",
            "/dev/video2",
            run_rgb,
            run_ir
        ));

        let (run_rgb, run_ir) = auth_streams("primary", "/dev/video2", false, true);
        assert!(and_policy_unsatisfiable(
            "and",
            "primary",
            "/dev/video2",
            run_rgb,
            run_ir
        ));
    }

    #[test]
    fn and_policy_is_satisfiable_with_both_spectra_enrolled() {
        let (run_rgb, run_ir) = auth_streams("primary", "/dev/video2", true, true);
        assert!(!and_policy_unsatisfiable(
            "and",
            "primary",
            "/dev/video2",
            run_rgb,
            run_ir
        ));
    }

    #[test]
    fn single_camera_hosts_are_not_blocked_by_the_and_policy() {
        let (run_rgb, run_ir) = auth_streams("primary", "", true, false);
        assert!(!and_policy_unsatisfiable(
            "and", "primary", "", run_rgb, run_ir
        ));

        let (run_rgb, run_ir) = auth_streams("", "/dev/video2", false, true);
        assert!(!and_policy_unsatisfiable(
            "and",
            "",
            "/dev/video2",
            run_rgb,
            run_ir
        ));
    }

    #[test]
    fn other_policies_still_allow_a_single_spectrum() {
        for policy in ["or", "fallback_on_dark", "default", ""] {
            let (run_rgb, run_ir) = auth_streams("primary", "/dev/video2", true, false);
            assert!(
                !and_policy_unsatisfiable(policy, "primary", "/dev/video2", run_rgb, run_ir),
                "{policy}"
            );
        }
    }

    #[test]
    fn hybrid_or_and_policies_require_the_configured_successes() {
        for rgb_status in [CaptureStatus::Usable, CaptureStatus::TooDark] {
            assert!(hybrid_auth_passed(
                "or", true, true, true, rgb_status, true, false
            ));
            assert!(hybrid_auth_passed(
                "or", true, true, true, rgb_status, false, true
            ));
            assert!(!hybrid_auth_passed(
                "and", true, true, true, rgb_status, true, false
            ));
            assert!(hybrid_auth_passed(
                "and", true, true, true, rgb_status, true, true
            ));
        }
    }

    #[test]
    fn hybrid_fallback_uses_ir_only_after_rgb_is_unavailable() {
        assert!(!hybrid_auth_passed(
            "fallback",
            true,
            true,
            false,
            CaptureStatus::Unused,
            false,
            true
        ));
        assert!(hybrid_auth_passed(
            "fallback",
            true,
            true,
            true,
            CaptureStatus::TooDark,
            false,
            true
        ));
        assert!(!hybrid_auth_passed(
            "fallback",
            true,
            true,
            true,
            CaptureStatus::NoFace,
            false,
            true
        ));
        assert!(!hybrid_auth_passed(
            "fallback",
            true,
            true,
            true,
            CaptureStatus::Usable,
            false,
            true
        ));
    }

    #[test]
    fn hybrid_fallback_yields_dark_rgb_to_ir_immediately() {
        for policy in ["fallback_on_dark", "default", ""] {
            assert!(should_yield_rgb_to_ir(policy, true, CaptureStatus::TooDark));
        }
        assert!(!should_yield_rgb_to_ir(
            "fallback_on_dark",
            false,
            CaptureStatus::TooDark
        ));
        assert!(!should_yield_rgb_to_ir(
            "fallback_on_dark",
            true,
            CaptureStatus::NoFace
        ));
        assert!(!should_yield_rgb_to_ir("or", true, CaptureStatus::TooDark));
        assert!(!should_yield_rgb_to_ir("and", true, CaptureStatus::TooDark));
    }

    #[test]
    fn single_spectrum_authentication_ignores_the_other_result() {
        assert!(hybrid_auth_passed(
            "and",
            true,
            false,
            true,
            CaptureStatus::Usable,
            true,
            false
        ));
        assert!(hybrid_auth_passed(
            "and",
            false,
            true,
            false,
            CaptureStatus::Unused,
            false,
            true
        ));
        assert!(!hybrid_auth_passed(
            "or",
            false,
            false,
            false,
            CaptureStatus::Unused,
            true,
            true
        ));
    }
}
