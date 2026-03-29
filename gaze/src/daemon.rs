use ndarray::Array1;
use opencv::core::Mat;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use tokio::time::{Duration, sleep};
use tracing::{error, info};
use zbus::zvariant::Value;
use zbus::{fdo, interface, message::Header, object_server::SignalContext};

use crate::align::align_face;
use crate::recognize::FaceRecognizer;
use crate::users::{UserDatabase, UserDbError};
use gaze_core::camera::Camera;
use gaze_core::config::Config;
use gaze_core::dbus::{CaptureStatus, EnrollPrompt, VerifyResult};
use gaze_core::face::FaceChecker;

const CONFIG_PATH: &str = "/etc/gaze/config.toml";
const POLKIT_ACTION_MANAGE_FACES: &str = "com.gundulabs.gaze.manage-faces";
const POLKIT_ACTION_MANAGE_CONFIG: &str = "com.gundulabs.gaze.manage-config";

#[derive(Clone)]
pub struct ClaimState {
    pub username: String,
    pub sender: String,
}

type FaceData = (Array1<f32>, [f32; 4]);

pub struct AuthDaemon {
    pub checker: Arc<Mutex<FaceChecker>>,
    pub recognizer: Arc<Mutex<FaceRecognizer>>,
    pub db: Arc<Mutex<UserDatabase>>,
    pub threshold: Arc<Mutex<f32>>,
    pub camera_config: Arc<Mutex<String>>,
    pub claim_state: Arc<Mutex<Option<ClaimState>>>,
    pub active_cancel: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    pub rt_handle: tokio::runtime::Handle,
}

impl AuthDaemon {
    fn map_user_db_error(err: UserDbError) -> fdo::Error {
        match err {
            UserDbError::UserNotFound(msg) => fdo::Error::FileNotFound(msg),
            UserDbError::FaceNotFound(msg) => fdo::Error::FileNotFound(msg),
            UserDbError::FaceExists(msg) => fdo::Error::FileExists(msg),
            UserDbError::Io(io_err) => fdo::Error::Failed(io_err.to_string()),
        }
    }

    fn process_frame(
        checker: &mut FaceChecker,
        recognizer: &mut FaceRecognizer,
        frame: &Mat,
    ) -> anyhow::Result<(CaptureStatus, Option<FaceData>)> {
        let (status, result_opt) = checker.capture_status(frame)?;

        if matches!(status, CaptureStatus::Clipped) {
            return Ok((status, None));
        }

        if let Some(res) = result_opt {
            let Some(kpss) = &res.kpss else {
                return Ok((status, None));
            };
            let Some(mat_rgb) = &res.mat_rgb else {
                return Ok((status, None));
            };

            let aligned = align_face(mat_rgb, kpss, 0)?;

            let embedding = recognizer.get_embedding(&aligned)?;

            let (x1, y1, x2, y2) = res.bbox.unwrap_or((0.0, 0.0, 0.0, 0.0));
            Ok((status, Some((embedding, [x1, y1, x2, y2]))))
        } else {
            Ok((status, None))
        }
    }

    async fn ensure_authorized(header: &Header<'_>, action_id: &str) -> fdo::Result<()> {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;

        let conn = zbus::Connection::system()
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to connect to system bus: {e}")))?;
        let authority = zbus::Proxy::new(
            &conn,
            "org.freedesktop.PolicyKit1",
            "/org/freedesktop/PolicyKit1/Authority",
            "org.freedesktop.PolicyKit1.Authority",
        )
        .await
        .map_err(|e| fdo::Error::Failed(format!("Failed to create polkit proxy: {e}")))?;

        let mut subject_details: HashMap<&str, Value<'_>> = HashMap::new();
        subject_details.insert("name", sender.as_str().into());

        let subject = ("system-bus-name", subject_details);
        let details: HashMap<&str, &str> = HashMap::new();
        let flags = 1u32; // AllowUserInteraction
        let cancellation_id = "";

        let (is_authorized, _is_challenge, _ret_details): (bool, bool, HashMap<String, String>) =
            authority
                .call(
                    "CheckAuthorization",
                    &(subject, action_id, details, flags, cancellation_id),
                )
                .await
                .map_err(|e| {
                    fdo::Error::Failed(format!("PolicyKit CheckAuthorization failed: {e}"))
                })?;

        if !is_authorized {
            return Err(fdo::Error::AccessDenied(format!(
                "Authorization denied for action '{action_id}'"
            )));
        }

        Ok(())
    }

    async fn check_claim(&self, header: &Header<'_>) -> fdo::Result<String> {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;

        let state = self.claim_state.lock().await;
        if let Some(claim) = &*state {
            if claim.sender == sender {
                return Ok(claim.username.clone());
            } else {
                return Err(fdo::Error::Failed(
                    "Daemon is claimed by another process".into(),
                ));
            }
        }
        Err(fdo::Error::Failed("Daemon is not claimed".into()))
    }

    fn cancel_active_tasks(&self) {
        if let Ok(mut cancel) = self.active_cancel.try_lock()
            && let Some(sender) = cancel.take()
        {
            let _ = sender.send(());
        }
    }
}

pub async fn get_active_session_uid() -> anyhow::Result<u32> {
    let connection = zbus::Connection::system().await?;
    let proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        "/org/freedesktop/login1/seat/seat0",
        "org.freedesktop.login1.Seat",
    )
    .await?;
    let active_session: (String, zbus::zvariant::ObjectPath) =
        proxy.get_property("ActiveSession").await?;

    let session_proxy = zbus::Proxy::new(
        &connection,
        "org.freedesktop.login1",
        active_session.1,
        "org.freedesktop.login1.Session",
    )
    .await?;
    let user: (u32, zbus::zvariant::ObjectPath) = session_proxy.get_property("User").await?;

    Ok(user.0)
}

#[interface(name = "com.gundulabs.Gaze")]
impl AuthDaemon {
    async fn claim(&self, #[zbus(header)] header: Header<'_>, username: String) -> fdo::Result<()> {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;

        let mut state = self.claim_state.lock().await;
        if let Some(existing) = &*state {
            if existing.sender == sender {
                return Ok(());
            }
            return Err(fdo::Error::Failed(
                "Device already claimed by another interface".into(),
            ));
        }

        info!(sender = %sender, username = %username, "Claimed daemon");
        *state = Some(ClaimState { username, sender });
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

            self.cancel_active_tasks();
            *state = None;
            info!(sender = %sender, "Released daemon");
            Ok(())
        } else {
            Err(fdo::Error::Failed("Daemon not claimed".into()))
        }
    }

    async fn verify_start(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        #[zbus(header)] header: Header<'_>,
        _face_name: String,
    ) -> fdo::Result<()> {
        let username = self.check_claim(&header).await?;
        self.cancel_active_tasks();

        let (tx, mut rx) = oneshot::channel();
        *self.active_cancel.lock().await = Some(tx);

        let checker_arc = self.checker.clone();
        let recognizer_arc = self.recognizer.clone();
        let db_arc = self.db.clone();
        let threshold_arc = self.threshold.clone();
        let camera_config = self.camera_config.lock().await.clone();

        let conn = ctxt.connection().clone();
        let path = ctxt.path().to_owned();

        self.rt_handle.spawn(async move {
            let ctxt = SignalContext::new(&conn, path).unwrap();


            let mut cam = match Camera::open(&camera_config) {
                Ok(c) => c,
                Err(e) => {
                    error!("Camera error: {e}");
                    let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new()).await;
                    return;
                }
            };

            info!("VerifyStart: sensing faces for user {}", username);

            let mut last_capture_status: Option<CaptureStatus> = None;
            loop {
                tokio::select! {
                    _ = &mut rx => {
                        info!("VerifyStart: cancelled");
                        let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new()).await;
                        break;
                    }
                    _ = tokio::task::yield_now() => {}
                }

                let frame = match cam.capture_frame() {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                let threshold = *threshold_arc.lock().await;

                let (_status, embed_opt) = match Self::process_and_emit_status(&ctxt, &checker_arc, &recognizer_arc, &frame, &mut last_capture_status).await {
                    Ok(res) => res,
                    Err(_) => continue,
                };

                if let Some((embed, _)) = embed_opt {
                    let db = db_arc.lock().await;

                    match db.match_faces(&username, &embed, threshold) {
                        Ok(scores) => {
                            let matched = scores.iter().any(|(_, _, _, passed, _)| *passed);
                            let faces: Vec<(String, f64, f64, bool, u32)> = scores
                                .iter()
                                .map(|(name, sim, pct, passed, count)| {
                                    (name.clone(), *sim as f64, *pct as f64, *passed, *count)
                                })
                                .collect();

                            let result = if matched {
                                info!("VerifyStart: MATCHED!");
                                VerifyResult::VerifyMatch
                            } else {
                                info!("VerifyStart: no match");
                                VerifyResult::VerifyNoMatch
                            };
                            let _ = Self::verify_status(&ctxt, result, faces).await;
                            break;
                        }
                        Err(e) => {
                            error!("DB error during verify: {e}");
                            let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new()).await;
                            break;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    async fn verify_stop(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        self.check_claim(&header).await?;
        self.cancel_active_tasks();
        Ok(())
    }

    async fn enroll_start(
        &self,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        #[zbus(header)] header: Header<'_>,
        face_name: String,
    ) -> fdo::Result<()> {
        let username = self.check_claim(&header).await?;
        self.cancel_active_tasks();

        if face_name.trim().is_empty() {
            return Err(fdo::Error::InvalidArgs("Face name cannot be empty".into()));
        }

        let (tx, mut rx) = oneshot::channel();
        *self.active_cancel.lock().await = Some(tx);

        let checker_arc = self.checker.clone();
        let recognizer_arc = self.recognizer.clone();
        let db_arc = self.db.clone();
        let camera_config = self.camera_config.lock().await.clone();

        let conn = ctxt.connection().clone();
        let path = ctxt.path().to_owned();

        self.rt_handle.spawn(async move {
            let ctxt = SignalContext::new(&conn, path).unwrap();


            let mut cam = match Camera::open(&camera_config) {
                Ok(c) => c,
                Err(e) => {
                    error!("Camera error: {e}");
                    let _ = Self::enroll_status(&ctxt, &face_name, 0, 5, true, EnrollPrompt::Cancelled, -1.0).await;
                    return;
                }
            };

            let template_id = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|_| "0".to_string());

            info!("EnrollStart: capturing faces for {}, target: {}, template: {}", username, face_name, template_id);

            let prompts = [
                EnrollPrompt::LookStraight,
                EnrollPrompt::LookUp,
                EnrollPrompt::LookDown,
                EnrollPrompt::LookLeft,
                EnrollPrompt::LookRight,
            ];
            let mut last_enroll_prompt: Option<EnrollPrompt> = None;
            let mut last_capture_status: Option<CaptureStatus> = None;
            let mut captured_embeddings: Vec<Array1<f32>> = Vec::new();
            let mut countdown_start: Option<std::time::Instant> = None;
            let countdown_duration = std::time::Duration::from_millis(1500);
            let max_steps = 5u32;

            loop {
                tokio::select! {
                    _ = &mut rx => {
                        info!("EnrollStart: cancelled");
                        let _ = Self::enroll_status(&ctxt, &face_name, 0, max_steps, true, EnrollPrompt::Cancelled, -1.0).await;
                        break;
                    }
                    _ = tokio::task::yield_now() => {}
                }

                let current_step_idx = captured_embeddings.len();
                let prompt = prompts[current_step_idx];

                let frame = match cam.capture_frame() {
                    Ok(f) => f,
                    Err(_) => continue,
                };

                let (status, embed_opt) = match Self::process_and_emit_status(&ctxt, &checker_arc, &recognizer_arc, &frame, &mut last_capture_status).await {
                    Ok(res) => res,
                    Err(_) => {
                        countdown_start = None;
                        continue;
                    }
                };

                let Some((embed, _bbox)) = embed_opt else {
                    countdown_start = None;
                    continue;
                };

                macro_rules! send_enroll_status {
                    ($msg:expr, $rem:expr) => {
                        if Some($msg) != last_enroll_prompt || $rem > 0.0 {
                            let _ = Self::enroll_status(&ctxt, &face_name, current_step_idx as u32, max_steps, false, $msg, $rem).await;
                            last_enroll_prompt = Some($msg);
                        }
                    }
                }

                match status {
                    CaptureStatus::Ready => {
                        if countdown_start.is_none() {
                            countdown_start = Some(std::time::Instant::now());
                        }

                        let elapsed = countdown_start.unwrap().elapsed();
                        if elapsed < countdown_duration {
                            let remaining = (countdown_duration - elapsed).as_secs_f64();
                            send_enroll_status!(prompt, remaining);
                            continue;
                        }

                        captured_embeddings.push(embed);
                        let new_count = captured_embeddings.len() as u32;
                        countdown_start = None;
                        last_enroll_prompt = None;

                        if new_count == max_steps {
                            info!("All angles captured! Saving template...");
                            let mut db = db_arc.lock().await;
                            match db.add_template(&username, &face_name, &template_id, captured_embeddings) {
                                Ok(_) => {
                                    info!("Template saved successfully!");
                                    let _ = Self::enroll_status(&ctxt, &face_name, max_steps, max_steps, true, EnrollPrompt::Completed, 0.0).await;
                                    break;
                                }
                                Err(e) => {
                                    error!("DB error saving template: {}", e);
                                    let _ = Self::enroll_status(&ctxt, &face_name, max_steps, max_steps, true, EnrollPrompt::DbFailed, -1.0).await;
                                    break;
                                }
                            }
                        } else {
                            info!("Angle progress: {}/{}", new_count, max_steps);
                            let _ = Self::enroll_status(&ctxt, &face_name, new_count, max_steps, false, EnrollPrompt::Captured, 0.0).await;
                            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                        }
                    }
                    _ => {
                        countdown_start = None;
                        send_enroll_status!(prompt, 0.0);
                    }
                }
            }
        });

        Ok(())
    }

    async fn enroll_stop(&self, #[zbus(header)] header: Header<'_>) -> fdo::Result<()> {
        self.check_claim(&header).await?;
        self.cancel_active_tasks();
        Ok(())
    }

    async fn list_faces(&self, username: String) -> fdo::Result<Vec<(String, u32)>> {
        let db = self.db.lock().await;
        db.list_faces(&username).map_err(Self::map_user_db_error)
    }

    async fn delete_face(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
        face_name: String,
    ) -> fdo::Result<bool> {
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_FACES).await?;
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
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_FACES).await?;
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
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_FACES).await?;
        let mut db = self.db.lock().await;
        db.clear_user(&username).map_err(Self::map_user_db_error)?;
        Ok(true)
    }

    async fn get_config(&self) -> fdo::Result<Config> {
        let config = Config::load_from(CONFIG_PATH)
            .map_err(|e| fdo::Error::Failed(format!("Failed to load config: {e}")))?;
        Ok(config)
    }

    async fn set_config(
        &self,
        #[zbus(header)] header: Header<'_>,
        config: Config,
    ) -> fdo::Result<bool> {
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_CONFIG).await?;

        config
            .save_to(CONFIG_PATH)
            .map_err(|e| fdo::Error::Failed(format!("Failed to save config: {e}")))?;

        info!("Config updated; scheduling daemon restart");
        self.rt_handle.spawn(async {
            sleep(Duration::from_millis(150)).await;
            std::process::exit(42);
        });

        Ok(true)
    }

    #[zbus(signal)]
    async fn verify_status(
        ctxt: &SignalContext<'_>,
        result: VerifyResult,
        faces: Vec<(String, f64, f64, bool, u32)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn face_status(ctxt: &SignalContext<'_>, status: CaptureStatus) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn enroll_status(
        ctxt: &SignalContext<'_>,
        face_name: &str,
        progress: u32,
        max: u32,
        is_done: bool,
        msg: EnrollPrompt,
        time_remaining: f64,
    ) -> zbus::Result<()>;
}

impl AuthDaemon {
    async fn process_and_emit_status(
        ctxt: &SignalContext<'_>,
        checker_arc: &Arc<Mutex<FaceChecker>>,
        recognizer_arc: &Arc<Mutex<FaceRecognizer>>,
        frame: &Mat,
        last_status: &mut Option<CaptureStatus>,
    ) -> anyhow::Result<(CaptureStatus, Option<FaceData>)> {
        let (status, embed_opt) = {
            let mut checker = checker_arc.lock().await;
            let mut recognizer = recognizer_arc.lock().await;
            Self::process_frame(&mut checker, &mut recognizer, frame)?
        };

        if last_status.as_ref() != Some(&status) {
            let _ = Self::face_status(ctxt, status).await;
            *last_status = Some(status);
        }

        if embed_opt.is_none() && status == CaptureStatus::NoFace {
            anyhow::bail!("No face");
        }

        Ok((status, embed_opt))
    }
}
