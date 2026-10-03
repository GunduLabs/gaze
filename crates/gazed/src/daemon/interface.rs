// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use zbus::interface;

enum EnrollMsg {
    Status(usize, Spectrum, CaptureStatus),
    Captured(usize, Spectrum, Array1<f32>),
    Error(String),
}

#[interface(name = "com.gundulabs.Gaze")]
impl AuthDaemon {
    async fn register_extension(
        &self,
        #[zbus(header)] header: Header<'_>,
        active: bool,
    ) -> fdo::Result<()> {
        let caller_uid = Self::caller_uid(&header)
            .await
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let mut extensions = self.active_extensions.lock().await;
        extensions.insert(caller_uid, active);
        info!(caller_uid, active, "Registered extension status");
        Ok(())
    }

    async fn is_extension_active(
        &self,
        #[zbus(header)] header: Header<'_>,
        uid: u32,
    ) -> fdo::Result<bool> {
        let caller_uid = Self::caller_uid(&header).await?;
        if !Self::may_query_extension(caller_uid, uid) {
            return Err(fdo::Error::AccessDenied(
                "not permitted to query another user's extension state".into(),
            ));
        }
        let extensions = self.active_extensions.lock().await;
        let is_active = extensions.get(&uid).copied().unwrap_or(false);
        Ok(is_active)
    }

    async fn claim(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        username: String,
    ) -> fdo::Result<()> {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;

        let caller_uid = Self::caller_uid(&header).await?;
        let target_uid = Self::username_uid(&username)?;
        if caller_uid != 0 && caller_uid != target_uid {
            Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_FACES).await?;
        }

        if !Self::seat_camera_available(caller_uid, target_uid).await {
            return Err(fdo::Error::AccessDenied(
                "refusing face auth: the seat camera belongs to another user's session".into(),
            ));
        }

        let mut state = self.claim_state.lock().await;
        if let Some(existing) = &*state {
            if existing.sender == sender {
                return Ok(());
            }
            if caller_uid == 0 {
                self.cancel_active_tasks().await;
                info!(
                    sender = %sender,
                    previous_sender = %existing.sender,
                    "Root caller preempting existing daemon claim"
                );
            } else {
                return Err(fdo::Error::Failed(
                    "Device already claimed by another interface".into(),
                ));
            }
        }

        info!(
            sender = %sender,
            username = %username,
            target_uid,
            caller_uid,
            "Claimed daemon"
        );
        let epoch = CLAIM_EPOCH.fetch_add(1, Ordering::Relaxed);
        *state = Some(ClaimState {
            username,
            sender: sender.clone(),
            epoch,
        });
        drop(state);

        let claim_state = self.claim_state.clone();
        let active_cancel = self.active_cancel.clone();

        let timeout_sender = sender.clone();
        self.rt_handle.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(CLAIM_TIMEOUT_SECS)).await;
            if release_claim_epoch(&claim_state, &active_cancel, epoch).await {
                warn!(
                    sender = %timeout_sender,
                    timeout_secs = CLAIM_TIMEOUT_SECS,
                    "Claim timed out and was reclaimed; the client never released it"
                );
            }
        });

        let claim_state = self.claim_state.clone();
        let active_cancel = self.active_cancel.clone();
        let conn = conn.clone();
        let sender_for_check = sender.clone();

        // The watcher may have handled this sender's disappearance while the claim was still
        // being authorized, finding nothing to release, so confirm the owner once here.
        self.rt_handle.spawn(async move {
            let dbus = match fdo::DBusProxy::new(&conn).await {
                Ok(dbus) => dbus,
                Err(e) => {
                    warn!(
                        sender = %sender_for_check,
                        error = %e,
                        "No DBus proxy to confirm the claim owner; this claim will hold \
                         until it times out"
                    );
                    return;
                }
            };

            let watched = match BusName::try_from(sender_for_check.clone()) {
                Ok(watched) => watched,
                Err(e) => {
                    warn!(
                        sender = %sender_for_check,
                        error = %e,
                        "Unparsable claim sender; skipping the owner confirmation"
                    );
                    return;
                }
            };

            match dbus.name_has_owner(watched).await {
                Ok(false) => {
                    if release_claim_epoch(&claim_state, &active_cancel, epoch).await {
                        info!(
                            sender = %sender_for_check,
                            "Sender vanished while claiming, auto-releasing claim"
                        );
                    }
                }
                Ok(true) => {}
                Err(e) => warn!(
                    sender = %sender_for_check,
                    error = %e,
                    "Could not confirm the claim owner; a sender that vanished while \
                     claiming will hold the claim until it times out"
                ),
            }
        });

        Ok(())
    }

    async fn release(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;

        let mut state = self.claim_state.lock().await;
        if let Some(claim) = &*state {
            if claim.sender != sender {
                return Err(fdo::Error::Failed("Sender does not own the claim".into()));
            }

            self.cancel_active_tasks().await;
            *state = None;
            info!(sender = %sender, "Released daemon");
            Ok(())
        } else {
            Err(fdo::Error::Failed("Daemon not claimed".into()))
        }
    }

    async fn verify_start(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
        _face_name: String,
    ) -> fdo::Result<()> {
        self.start_verification(ctxt, header, None, false).await
    }

    async fn verify_start_for(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
        _face_name: String,
        pam_service: String,
    ) -> fdo::Result<()> {
        self.start_verification(ctxt, header, Some(pam_service), false)
            .await
    }

    async fn verify_start_for_keyring(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<()> {
        self.start_verification(ctxt, header, Some("gdm-face".into()), true)
            .await
    }

    async fn verify_start_for_kwallet(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
        pam_service: String,
    ) -> fdo::Result<()> {
        if !matches!(
            pam_service.as_str(),
            "sddm" | "plasmalogin" | "plasmalogin-fingerprint"
        ) {
            return Err(fdo::Error::InvalidArgs(
                "KWallet requires a KDE login service".into(),
            ));
        }
        self.start_verification(ctxt, header, Some(pam_service), true)
            .await
    }

    async fn kwallet_enabled(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<bool> {
        Self::ensure_config_read_access(&header).await?;
        Ok(self.current_config().await.storage.unlock_kwallet)
    }

    async fn keyring_enabled(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<bool> {
        Self::ensure_config_read_access(&header).await?;
        Ok(self.current_config().await.storage.unlock_gnome_keyring)
    }

    async fn verify_stop(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        self.check_claim(&header).await?;
        self.cancel_active_tasks().await;
        Ok(())
    }

    async fn enroll_start(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
        face_name: String,
    ) -> fdo::Result<()> {
        let claim = self.check_claim(&header).await?;
        let username = claim.username.clone();
        Self::ensure_face_write_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let signal_destination = Self::signal_destination(&claim.sender)?;
        self.cancel_active_tasks().await;

        UserDatabase::validate_face_name(&face_name).map_err(Self::map_user_db_error)?;

        let (tx, mut rx) = oneshot::channel();
        *self.active_cancel.lock().await = Some(tx);

        let detector_arc = self.detector.clone();
        let recognizer_rgb_arc = self.recognizer_rgb.clone();
        let recognizer_ir_arc = self.recognizer_ir.clone();
        let db_arc = self.db.clone();

        let config = self.current_config().await;
        let sources = resolve_configured_sources(&config.cameras);
        let rgb_device = sources.rgb;
        let ir_device = sources.ir;
        let ir_node = sources.ir_node;
        let emitter_enabled = config.cameras.emitter_enabled;
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

            let run_rgb = !rgb_device.is_empty();
            let run_ir = !ir_device.is_empty();

            if !run_rgb && !run_ir {
                error!("No cameras configured for enrollment");
                let _ = Self::enroll_status(&ctxt, &face_name, 0, 5, true, EnrollPrompt::Cancelled, -1.0).await;
                return;
            }

            let template_id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|_| "0".to_string());

            info!(
                "EnrollStart: capturing faces for {}, target: {}, template: {}, run_rgb: {}, run_ir: {}",
                username, face_name, template_id, run_rgb, run_ir
            );

            let prompts = [
                EnrollPrompt::LookStraight,
                EnrollPrompt::LookUp,
                EnrollPrompt::LookDown,
                EnrollPrompt::LookLeft,
                EnrollPrompt::LookRight,
            ];
            let max_steps = 5u32;

            let (enroll_tx, mut enroll_rx) = tokio::sync::mpsc::channel::<EnrollMsg>(10);
            let (preview_tx, mut preview_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
            let stream_preview = !gaze_vision::camera::preview_can_be_shared(&config.cameras);
            let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let completed_steps_atomic = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let rgb_captured_for_step = Arc::new(std::sync::atomic::AtomicBool::new(false));

            let mut rgb_thread = None;
            if run_rgb {
                let stop_clone = stop_flag.clone();
                let tx = enroll_tx.clone();
                let detector_arc = detector_arc.clone();
                let config_clone = config.clone();
                let recognizer_rgb_arc = recognizer_rgb_arc.clone();
                let completed_steps_clone = completed_steps_atomic.clone();
                let rgb_device_clone = rgb_device.clone();
                let rgb_captured_for_step_clone = rgb_captured_for_step.clone();
                let preview_tx_clone = preview_tx.clone();

                rgb_thread = Some(std::thread::spawn(move || {
                    let mut checker = FaceChecker::new(detector_arc, &config_clone, Spectrum::Rgb, true);
                    let mut preview = if stream_preview {
                        PreviewStream::new(preview_tx_clone)
                    } else {
                        PreviewStream::disabled()
                    };
                    let mut pose_baseline = None;

                    // Cameras like the Logitech Brio 4K cannot stream RGB and IR at once, so
                    // dual-spectrum mode releases the RGB camera once a step is captured.
                    if run_ir {
                        let mut dead_streams = 0u32;

                        'steps: loop {
                            if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                                return;
                            }
                            let step = completed_steps_clone.load(std::sync::atomic::Ordering::Relaxed) as usize;
                            if step >= max_steps as usize {
                                return;
                            }
                            if rgb_captured_for_step_clone.load(std::sync::atomic::Ordering::Relaxed) {
                                std::thread::sleep(Duration::from_millis(50));
                                continue;
                            }

                            let mut cam = match Camera::open_privileged(&rgb_device_clone) {
                                Ok(c) => c,
                                Err(e) => {
                                    dead_streams += 1;
                                    if dead_streams >= 3 {
                                        let _ = tx.blocking_send(EnrollMsg::Error(format!("RGB Camera open error: {e}")));
                                        return;
                                    }
                                    std::thread::sleep(Duration::from_millis(200));
                                    continue;
                                }
                            };
                            let mut pose_stability = EnrollmentPoseStability::default();

                            while let Some(frame) = cam.next_interruptible(&stop_clone) {
                                if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                                let current_step = completed_steps_clone.load(std::sync::atomic::Ordering::Relaxed) as usize;
                                if current_step >= max_steps as usize {
                                    return;
                                }
                                if current_step != step {
                                    continue 'steps;
                                }

                                preview.offer(&frame);

                                let prompt = prompts[current_step];

                                let (status, result_opt) = {
                                    let mut recognizer = recognizer_rgb_arc.blocking_lock();
                                    match process_frame_sync(&mut checker, &mut recognizer, &frame, false) {
                                        Ok(res) => res,
                                        Err(_) => (CaptureStatus::NoFace, None),
                                    }
                                };

                                let _ = tx.try_send(EnrollMsg::Status(current_step, Spectrum::Rgb, status));

                                if status == CaptureStatus::Usable && let Some(data) = result_opt {
                                    let is_stable = pose_stability.update(prompt, data.yaw, data.pitch);
                                    let pose_matches = enrollment_pose_matches(
                                        prompt,
                                        data.yaw,
                                        data.pitch,
                                        pose_baseline,
                                    );

                                    if is_stable && pose_matches {
                                        if prompt == EnrollPrompt::LookStraight {
                                            pose_baseline = Some((data.yaw, data.pitch));
                                        }
                                        rgb_captured_for_step_clone.store(true, std::sync::atomic::Ordering::Relaxed);
                                        let _ = tx.blocking_send(EnrollMsg::Captured(current_step, Spectrum::Rgb, data.embedding));
                                        dead_streams = 0;
                                        continue 'steps;
                                    }
                                } else {
                                    pose_stability.reset();
                                }
                            }

                            dead_streams += 1;
                            if dead_streams >= 3 {
                                let _ = tx.blocking_send(EnrollMsg::Error(
                                    "RGB camera stream stopped unexpectedly".into(),
                                ));
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }

                    let mut cam = match Camera::open_privileged(&rgb_device_clone) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(EnrollMsg::Error(format!("RGB Camera open error: {e}")));
                            return;
                        }
                    };

                    let mut last_processed_step = 999;
                    let mut captured_for_step = false;
                    let mut pose_stability = EnrollmentPoseStability::default();

                    while let Some(frame) = cam.next_interruptible(&stop_clone) {
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        let current_step = completed_steps_clone.load(std::sync::atomic::Ordering::Relaxed) as usize;
                        if current_step >= max_steps as usize {
                            break;
                        }

                        if current_step != last_processed_step {
                            last_processed_step = current_step;
                            captured_for_step = false;
                            pose_stability.reset();
                        }

                        if captured_for_step {
                            std::thread::sleep(Duration::from_millis(100));
                            continue;
                        }

                        preview.offer(&frame);

                        let prompt = prompts[current_step];

                        let (status, result_opt) = {
                            let mut recognizer = recognizer_rgb_arc.blocking_lock();
                            match process_frame_sync(&mut checker, &mut recognizer, &frame, false) {
                                Ok(res) => res,
                                Err(_) => (CaptureStatus::NoFace, None),
                            }
                        };

                        let _ = tx.try_send(EnrollMsg::Status(current_step, Spectrum::Rgb, status));

                        if status == CaptureStatus::Usable && let Some(data) = result_opt {
                            let is_stable = pose_stability.update(prompt, data.yaw, data.pitch);
                            let pose_matches = enrollment_pose_matches(
                                prompt,
                                data.yaw,
                                data.pitch,
                                pose_baseline,
                            );

                            if is_stable && pose_matches {
                                if prompt == EnrollPrompt::LookStraight {
                                    pose_baseline = Some((data.yaw, data.pitch));
                                }
                                let _ = tx.blocking_send(EnrollMsg::Captured(current_step, Spectrum::Rgb, data.embedding));
                                captured_for_step = true;
                            }
                        } else {
                            pose_stability.reset();
                        }
                    }

                    if !stop_clone.load(std::sync::atomic::Ordering::Relaxed)
                        && completed_steps_clone.load(std::sync::atomic::Ordering::Relaxed) < max_steps
                    {
                        let _ = tx.blocking_send(EnrollMsg::Error(
                            "RGB camera stream stopped unexpectedly".into(),
                        ));
                    }
                }));
            }

            let mut ir_thread = None;
            if run_ir {
                let stop_clone = stop_flag.clone();
                let tx = enroll_tx.clone();
                let detector_arc = detector_arc.clone();
                let config_clone = config.clone();
                let recognizer_ir_arc = recognizer_ir_arc.clone();
                let completed_steps_clone = completed_steps_atomic.clone();
                let ir_device_clone = ir_device.clone();
                let ir_node_clone = ir_node.clone();
                let rgb_captured_for_step_clone = rgb_captured_for_step.clone();
                let preview_tx_clone = preview_tx.clone();

                ir_thread = Some(std::thread::spawn(move || {
                    let mut checker = FaceChecker::new(detector_arc, &config_clone, Spectrum::Ir, true);
                    let mut dark_gate = IrDarkFrameGate::new(config_clone.cameras.dark_luma_threshold);
                    let mut preview = if stream_preview {
                        PreviewStream::new(preview_tx_clone)
                    } else {
                        PreviewStream::disabled()
                    };

                    // Dual-spectrum mode waits for RGB to capture and release the camera, then
                    // holds IR just long enough for one lit frame; RGB already checked the pose.
                    if run_rgb {
                        let mut captured_step = usize::MAX;
                        let mut dead_streams = 0u32;

                        'steps: loop {
                            if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                                return;
                            }
                            let step = completed_steps_clone.load(std::sync::atomic::Ordering::Relaxed) as usize;
                            if step >= max_steps as usize {
                                return;
                            }
                            if step == captured_step
                                || !rgb_captured_for_step_clone.load(std::sync::atomic::Ordering::Relaxed)
                            {
                                std::thread::sleep(Duration::from_millis(50));
                                continue;
                            }

                            // Realtek switches mode before the exact IR stream format is
                            // negotiated; changing format afterwards silently restores RGB.
                            let mut emitter = EmitterGuard::engage(
                                &CameraKind::Ir { source: ir_device_clone.clone(), node: ir_node_clone.clone() },
                                emitter_enabled
                            );
                            let mut cam = match Camera::open_ir_privileged(&ir_device_clone) {
                                Ok(c) => c,
                                Err(e) => {
                                    dead_streams += 1;
                                    if dead_streams >= 3 {
                                        let _ = tx.blocking_send(EnrollMsg::Error(format!("IR Camera open error: {e}")));
                                        return;
                                    }
                                    std::thread::sleep(Duration::from_millis(200));
                                    continue;
                                }
                            };

                            while let Some(frame) = cam.next_interruptible(&stop_clone) {
                                if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                                emitter.stream_started();
                                let current_step = completed_steps_clone.load(std::sync::atomic::Ordering::Relaxed) as usize;
                                if current_step >= max_steps as usize {
                                    return;
                                }
                                if current_step != step {
                                    continue 'steps;
                                }

                                match dark_gate.classify(&frame) {
                                    IrFrameKind::Lit => {}
                                    IrFrameKind::StrobeDark => continue,
                                    IrFrameKind::EmitterDark => {
                                        let _ = tx.try_send(EnrollMsg::Status(current_step, Spectrum::Ir, CaptureStatus::TooDark));
                                        continue;
                                    }
                                }

                                preview.offer(&frame);

                                let (status, result_opt) = {
                                    let mut recognizer = recognizer_ir_arc.blocking_lock();
                                    match process_frame_sync(&mut checker, &mut recognizer, &frame, false) {
                                        Ok(res) => res,
                                        Err(_) => (CaptureStatus::NoFace, None),
                                    }
                                };

                                let _ = tx.try_send(EnrollMsg::Status(current_step, Spectrum::Ir, status));

                                if status == CaptureStatus::Usable && let Some(data) = result_opt {
                                    let _ = tx.blocking_send(EnrollMsg::Captured(current_step, Spectrum::Ir, data.embedding));
                                    captured_step = step;
                                    dead_streams = 0;
                                    continue 'steps;
                                }
                            }

                            dead_streams += 1;
                            if dead_streams >= 3 {
                                let _ = tx.blocking_send(EnrollMsg::Error(
                                    "IR camera stream stopped unexpectedly".into(),
                                ));
                                return;
                            }
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }

                    let mut emitter = EmitterGuard::engage(
                        &CameraKind::Ir { source: ir_device_clone.clone(), node: ir_node_clone.clone() },
                        emitter_enabled
                    );

                    let mut cam = match Camera::open_ir_privileged(&ir_device_clone) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(EnrollMsg::Error(format!("IR Camera open error: {e}")));
                            return;
                        }
                    };

                    let mut last_processed_step = 999;
                    let mut captured_for_step = false;
                    let mut pose_stability = EnrollmentPoseStability::default();
                    let mut pose_baseline = None;

                    while let Some(frame) = cam.next_interruptible(&stop_clone) {
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                        emitter.stream_started();
                        let current_step = completed_steps_clone.load(std::sync::atomic::Ordering::Relaxed) as usize;
                        if current_step >= max_steps as usize {
                            break;
                        }

                        if current_step != last_processed_step {
                            last_processed_step = current_step;
                            captured_for_step = false;
                            pose_stability.reset();
                        }

                        if captured_for_step {
                            std::thread::sleep(Duration::from_millis(100));
                            continue;
                        }

                        match dark_gate.classify(&frame) {
                            IrFrameKind::Lit => {}
                            IrFrameKind::StrobeDark => continue,
                            IrFrameKind::EmitterDark => {
                                let _ = tx.try_send(EnrollMsg::Status(current_step, Spectrum::Ir, CaptureStatus::TooDark));
                                continue;
                            }
                        }

                        preview.offer(&frame);

                        let prompt = prompts[current_step];

                        let (status, result_opt) = {
                            let mut recognizer = recognizer_ir_arc.blocking_lock();
                            match process_frame_sync(&mut checker, &mut recognizer, &frame, false) {
                                Ok(res) => res,
                                Err(_) => (CaptureStatus::NoFace, None),
                            }
                        };

                        let _ = tx.try_send(EnrollMsg::Status(current_step, Spectrum::Ir, status));

                        if status == CaptureStatus::Usable && let Some(data) = result_opt {
                            let is_stable = pose_stability.update(prompt, data.yaw, data.pitch);
                            let pose_matches = enrollment_pose_matches(
                                prompt,
                                data.yaw,
                                data.pitch,
                                pose_baseline,
                            );

                            if is_stable && pose_matches {
                                if prompt == EnrollPrompt::LookStraight {
                                    pose_baseline = Some((data.yaw, data.pitch));
                                }
                                let _ = tx.blocking_send(EnrollMsg::Captured(current_step, Spectrum::Ir, data.embedding));
                                captured_for_step = true;
                            }
                        } else {
                            pose_stability.reset();
                        }
                    }

                    if !stop_clone.load(std::sync::atomic::Ordering::Relaxed)
                        && completed_steps_clone.load(std::sync::atomic::Ordering::Relaxed) < max_steps
                    {
                        let _ = tx.blocking_send(EnrollMsg::Error(
                            "IR camera stream stopped unexpectedly".into(),
                        ));
                    }
                }));
            }

            drop(enroll_tx);
            drop(preview_tx);

            let mut completed_steps = 0;
            let mut has_rgb_for_step = false;
            let mut has_ir_for_step = false;
            let mut step_rgb_embed = None;
            let mut step_ir_embed = None;
            let mut captured_embeddings = Vec::new();

            let mut rgb_status = CaptureStatus::NoFace;
            let mut ir_status = CaptureStatus::NoFace;
            let mut last_emitted_status = None;

            let mut last_sent_prompt = None;
            let mut aborted = false;

            while completed_steps < max_steps as usize {
                let prompt = prompts[completed_steps];
                if last_sent_prompt != Some(prompt) {
                    let _ = Self::enroll_status(&ctxt, &face_name, completed_steps as u32, max_steps, false, prompt, 0.0).await;
                    last_sent_prompt = Some(prompt);
                }

                tokio::select! {
                    _ = &mut rx => {
                        info!("EnrollStart: cancelled");
                        let _ = Self::enroll_status(&ctxt, &face_name, completed_steps as u32, max_steps, true, EnrollPrompt::Cancelled, -1.0).await;
                        aborted = true;
                        break;
                    }
                    Some(jpeg) = preview_rx.recv() => {
                        let _ = Self::preview_frame(&ctxt, &jpeg).await;
                    }
                    msg_opt = enroll_rx.recv() => {
                        let Some(msg) = msg_opt else {
                            warn!("EnrollStart: all capture threads exited before enrollment finished");
                            let _ = Self::enroll_status(&ctxt, &face_name, completed_steps as u32, max_steps, true, EnrollPrompt::CameraFailed, -1.0).await;
                            aborted = true;
                            break;
                        };
                        match msg {
                            EnrollMsg::Status(step, spectrum, status) => {
                                if step != completed_steps {
                                    continue;
                                }
                                match spectrum {
                                    Spectrum::Rgb => rgb_status = status,
                                    Spectrum::Ir => ir_status = status,
                                }
                                let r_status = if has_rgb_for_step { CaptureStatus::NoFace } else { rgb_status };
                                let i_status = if has_ir_for_step { CaptureStatus::NoFace } else { ir_status };

                                Self::emit_effective_face_status(
                                    &ctxt,
                                    &mut last_emitted_status,
                                    r_status,
                                    i_status,
                                ).await;
                            }
                            EnrollMsg::Captured(step, spectrum, embed) => {
                                if step != completed_steps {
                                    continue;
                                }
                                match spectrum {
                                    Spectrum::Rgb => {
                                        has_rgb_for_step = true;
                                        step_rgb_embed = Some(embed);
                                    }
                                    Spectrum::Ir => {
                                        has_ir_for_step = true;
                                        step_ir_embed = Some(embed);
                                    }
                                }

                                let r_status = if has_rgb_for_step { CaptureStatus::NoFace } else { rgb_status };
                                let i_status = if has_ir_for_step { CaptureStatus::NoFace } else { ir_status };

                                Self::emit_effective_face_status(
                                    &ctxt,
                                    &mut last_emitted_status,
                                    r_status,
                                    i_status,
                                ).await;

                                let step_done = match (run_rgb, run_ir) {
                                    (true, true) => has_rgb_for_step && has_ir_for_step,
                                    (true, false) => has_rgb_for_step,
                                    (false, true) => has_ir_for_step,
                                    (false, false) => false,
                                };

                                if step_done {
                                    if let Some(emb) = step_rgb_embed.take() {
                                        captured_embeddings.push((emb, Spectrum::Rgb));
                                    }
                                    if let Some(emb) = step_ir_embed.take() {
                                        captured_embeddings.push((emb, Spectrum::Ir));
                                    }

                                     has_rgb_for_step = false;
                                     has_ir_for_step = false;
                                     rgb_captured_for_step.store(false, std::sync::atomic::Ordering::Relaxed);

                                    completed_steps += 1;
                                    completed_steps_atomic.store(completed_steps as u32, std::sync::atomic::Ordering::Relaxed);

                                    let _ = Self::enroll_status(&ctxt, &face_name, completed_steps as u32, max_steps, false, EnrollPrompt::Captured, 0.0).await;
                                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                                }
                            }
                            EnrollMsg::Error(e) => {
                                error!("Enrollment error: {e}");
                                let _ = Self::enroll_status(&ctxt, &face_name, completed_steps as u32, max_steps, true, EnrollPrompt::CameraFailed, -1.0).await;
                                aborted = true;
                                break;
                            }
                        }
                    }
                }
            }

            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
            if !aborted {
                let mut db = db_arc.lock().await;
                match db.add_template(&username, &face_name, &template_id, captured_embeddings) {
                    Ok(_) => {
                        info!("Template saved successfully!");
                        let _ = Self::enroll_status(&ctxt, &face_name, max_steps, max_steps, true, EnrollPrompt::Completed, 0.0).await;
                    }
                    Err(e) => {
                        error!("DB error saving template: {}", e);
                        let _ = Self::enroll_status(&ctxt, &face_name, max_steps, max_steps, true, EnrollPrompt::DbFailed, -1.0).await;
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

    async fn enroll_stop(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        self.check_claim(&header).await?;
        self.cancel_active_tasks().await;
        Ok(())
    }

    async fn list_faces(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
    ) -> fdo::Result<Vec<(String, u32, bool, bool)>> {
        Self::ensure_user_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let db = self.db.lock().await;
        db.list_faces(&username).map_err(Self::map_user_db_error)
    }

    async fn has_enrolled_faces(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
    ) -> fdo::Result<bool> {
        Self::ensure_user_query_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let db = self.db.lock().await;
        db.has_enrolled_faces(&username)
            .map_err(Self::map_user_db_error)
    }

    async fn is_camera_available(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<bool> {
        let caller_uid = Self::caller_uid(&header).await?;
        Ok(Self::seat_camera_available(caller_uid, caller_uid).await)
    }

    async fn benchmark(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<Vec<gaze_core::dbus::BenchmarkResult>> {
        if Self::benchmark_needs_authorization(Self::caller_uid(&header).await?) {
            Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_CONFIG).await?;
        }

        let Some(_slot) = BenchmarkSlot::acquire(&self.benchmark_running) else {
            return Err(fdo::Error::Failed(
                "RETRYABLE: a benchmark is already running".into(),
            ));
        };

        let detector_arc = self.detector.clone();
        let recognizer_rgb_arc = self.recognizer_rgb.clone();
        let recognizer_ir_arc = self.recognizer_ir.clone();
        let liveness_arc = self.liveness.clone();

        self.rt_handle
            .spawn_blocking(move || {
                run_inference_benchmark(
                    detector_arc,
                    recognizer_rgb_arc,
                    recognizer_ir_arc,
                    liveness_arc,
                )
            })
            .await
            .map_err(|e| fdo::Error::Failed(format!("benchmark task panicked: {e}")))?
    }

    async fn delete_face(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
        face_name: String,
    ) -> fdo::Result<bool> {
        Self::ensure_face_write_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let mut db = self.db.lock().await;
        db.remove_face(&username, &face_name)
            .map_err(Self::map_user_db_error)?;
        Ok(true)
    }

    async fn rename_face(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
        old_face_name: String,
        new_face_name: String,
    ) -> fdo::Result<bool> {
        Self::ensure_face_write_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let mut db = self.db.lock().await;
        db.rename_face(&username, &old_face_name, &new_face_name)
            .map_err(Self::map_user_db_error)?;
        Ok(true)
    }

    async fn delete_faces(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
    ) -> fdo::Result<bool> {
        Self::ensure_face_write_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let mut db = self.db.lock().await;
        db.clear_user(&username).map_err(Self::map_user_db_error)?;
        Ok(true)
    }

    async fn duress_locked(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
    ) -> fdo::Result<bool> {
        Self::ensure_user_query_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        Ok(self.duress_lockout.is_locked(&username))
    }

    async fn clear_duress(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
    ) -> fdo::Result<bool> {
        Self::ensure_user_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let cleared = self
            .duress_lockout
            .clear(&username)
            .map_err(|e| fdo::Error::Failed(format!("Failed to clear duress lockout: {e}")))?;
        if cleared {
            info!(
                "Cleared the duress lockout for {}; face authentication is available again",
                username
            );
        }
        Ok(cleared)
    }

    #[zbus(property)]
    async fn pam_internal(
        &self,
        #[zbus(header)] header: Option<Header<'_>>,
    ) -> fdo::Result<Vec<String>> {
        let header =
            header.ok_or_else(|| fdo::Error::Failed("No message header provided".to_string()))?;
        let owner = Self::pam_internal_read_owner(&header).await?;
        let sets = self.pam_internal.lock().await;
        let mut list: Vec<String> = sets
            .get(&owner)
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default();
        list.sort();
        Ok(list)
    }

    #[zbus(property)]
    async fn set_pam_internal(
        &self,
        #[zbus(header)] header: Option<Header<'_>>,
        services: Vec<String>,
    ) -> fdo::Result<()> {
        let header =
            header.ok_or_else(|| fdo::Error::Failed("No message header provided".to_string()))?;
        let owner = Self::pam_internal_write_owner(&header).await?;
        let mut sets = self.pam_internal.lock().await;
        let set = sets.entry(owner).or_default();
        set.clear();
        for s in services {
            let normalized = Self::normalize_pam_service(&s);
            if !normalized.is_empty() {
                set.insert(normalized);
            }
        }
        info!(uid = owner, services = ?set, "Updated PAM internal services list");
        Ok(())
    }

    async fn add_pam_internal(
        &self,
        #[zbus(header)] header: Header<'_>,
        service: String,
    ) -> fdo::Result<()> {
        let owner = Self::pam_internal_write_owner(&header).await?;
        let normalized = Self::normalize_pam_service(&service);
        if !normalized.is_empty() {
            let mut sets = self.pam_internal.lock().await;
            sets.entry(owner).or_default().insert(normalized.clone());
            info!(uid = owner, service = %normalized, "Added to PAM internal services");
        }
        Ok(())
    }

    async fn remove_pam_internal(
        &self,
        #[zbus(header)] header: Header<'_>,
        service: String,
    ) -> fdo::Result<()> {
        let owner = Self::pam_internal_write_owner(&header).await?;
        let normalized = Self::normalize_pam_service(&service);
        if !normalized.is_empty() {
            let mut sets = self.pam_internal.lock().await;
            if let Some(set) = sets.get_mut(&owner) {
                set.remove(&normalized);
            }
            info!(uid = owner, service = %normalized, "Removed from PAM internal services");
        }
        Ok(())
    }

    async fn clear_pam_internal(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        let owner = Self::pam_internal_write_owner(&header).await?;
        self.pam_internal.lock().await.remove(&owner);
        info!(uid = owner, "Cleared PAM internal services");
        Ok(())
    }

    #[zbus(property(emits_changed_signal = "invalidates"))]
    async fn config(&self, #[zbus(header)] header: Option<Header<'_>>) -> fdo::Result<DbusConfig> {
        let header =
            header.ok_or_else(|| fdo::Error::Failed("No message header provided".to_string()))?;
        Self::ensure_config_read_access(&header).await?;
        Ok(self.current_config().await.into())
    }

    #[zbus(property)]
    async fn set_config(
        &self,
        #[zbus(header)] header: Option<Header<'_>>,
        new_config: DbusConfig,
    ) -> fdo::Result<()> {
        let header =
            header.ok_or_else(|| fdo::Error::Failed("No message header provided".to_string()))?;
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_CONFIG).await?;

        let mut new_config: Config = new_config.into();
        // Legacy clients do not send these flags; preserve the existing opt-ins.
        let storage = self.current_config().await.storage;
        new_config.storage.unlock_gnome_keyring = storage.unlock_gnome_keyring;
        new_config.storage.unlock_kwallet = storage.unlock_kwallet;
        // A legacy client cannot see or clear the flag, so treat it as turning the feature off
        // rather than rejecting every later write with an error it cannot act on.
        if new_config.clamp_keyring() {
            warn!(
                "a legacy config update removed a keyring prerequisite; disabling keyring unlock"
            );
        }
        self.apply_config(new_config).await
    }

    async fn set_config_with_keyring(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
        config: zbus::zvariant::OwnedValue,
        unlock_gnome_keyring: bool,
    ) -> fdo::Result<()> {
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_CONFIG).await?;
        let mut config = gaze_core::dbus::config_update_from_property(config, unlock_gnome_keyring)
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        config.storage.unlock_kwallet = self.current_config().await.storage.unlock_kwallet;
        // This older client cannot clear a KWallet opt-in when removing prerequisites.
        if config.storage.validate_keyring(&config.liveness).is_err() {
            config.storage.unlock_kwallet = false;
        }
        self.apply_config(config).await?;
        self.config_invalidate(&ctxt).await.map_err(Into::into)
    }

    async fn set_config_with_wallets(
        &self,
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
        config: zbus::zvariant::OwnedValue,
        unlock_gnome_keyring: bool,
        unlock_kwallet: bool,
    ) -> fdo::Result<()> {
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_CONFIG).await?;
        let mut config = gaze_core::dbus::config_update_from_property(config, unlock_gnome_keyring)
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        config.storage.unlock_kwallet = unlock_kwallet;
        config
            .storage
            .validate_keyring(&config.liveness)
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;
        self.apply_config(config).await?;
        self.config_invalidate(&ctxt).await.map_err(Into::into)
    }

    async fn get_gdm_face_auth(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<bool> {
        Self::ensure_config_read_access(&header).await?;
        if let Some(enabled) = gdm_face_auth_from_dconf() {
            return Ok(enabled);
        }
        Ok(std::path::Path::new(GDM_FACE_OVERRIDE_PATH).exists())
    }

    async fn set_gdm_face_auth(
        &self,
        #[zbus(header)] header: Header<'_>,
        enabled: bool,
    ) -> fdo::Result<bool> {
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_GDM_PROFILE).await?;

        let path = std::path::Path::new(GDM_FACE_OVERRIDE_PATH);
        // Already in the requested state elsewhere, so don't write a read-only /etc.
        if !path.exists() && gdm_face_auth_from_dconf() == Some(enabled) {
            info!(enabled, "GDM face authentication already set outside Gaze");
            return Ok(enabled);
        }

        if enabled {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| gdm_override_error("create", parent, e))?;
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

        info!(enabled, "Updated GDM face authentication override");
        Ok(enabled)
    }

    #[zbus(signal)]
    pub(super) async fn verify_status(
        ctxt: &SignalEmitter<'_>,
        result: VerifyResult,
        faces: Vec<(String, f64, f64, bool, f64, f64, bool)>,
        rgb_status: CaptureStatus,
        ir_status: CaptureStatus,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(super) async fn verify_diagnostic(
        ctxt: &SignalEmitter<'_>,
        message: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(super) async fn face_status(
        ctxt: &SignalEmitter<'_>,
        status: CaptureStatus,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(super) async fn preview_frame(ctxt: &SignalEmitter<'_>, jpeg: &[u8]) -> zbus::Result<()>;

    #[zbus(signal)]
    pub(super) async fn enroll_status(
        ctxt: &SignalEmitter<'_>,
        face_name: &str,
        progress: u32,
        max: u32,
        is_done: bool,
        msg: EnrollPrompt,
        time_remaining: f64,
    ) -> zbus::Result<()>;
}
