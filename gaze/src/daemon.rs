use futures::StreamExt;
use ndarray::Array1;
use opencv::core::Mat;
use std::collections::HashMap;
use std::ffi::CString;
use std::ptr;
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use tracing::{error, info, warn};
use zbus::names::BusName;
use zbus::zvariant::OwnedValue;
use zbus::{fdo, interface, message::Header, object_server::SignalEmitter};

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
const CLAIM_TIMEOUT_SECS: u64 = 300;

#[derive(Clone)]
pub struct ClaimState {
    pub username: String,
    pub sender: String,
}

pub struct FaceData {
    pub embedding: Array1<f32>,
    pub bbox: [f32; 4],
    pub kpss: ndarray::Array3<f32>,
    pub yaw: f32,
    pub pitch: f32,
}

pub struct AuthDaemon {
    pub checker: Arc<Mutex<FaceChecker>>,
    pub recognizer: Arc<Mutex<FaceRecognizer>>,
    pub db: Arc<Mutex<UserDatabase>>,
    pub threshold: Arc<Mutex<f32>>,
    pub camera_config: Arc<Mutex<String>>,
    pub abort_if_ssh: Arc<Mutex<bool>>,
    pub abort_if_lid_closed: Arc<Mutex<bool>>,
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
            UserDbError::InvalidName(msg) => fdo::Error::InvalidArgs(msg),
            UserDbError::Io(io_err) => fdo::Error::Failed(io_err.to_string()),
        }
    }

    fn username_uid(username: &str) -> fdo::Result<u32> {
        UserDatabase::validate_username(username).map_err(Self::map_user_db_error)?;

        let c_username = CString::new(username)
            .map_err(|_| fdo::Error::InvalidArgs("username contains NUL byte".into()))?;
        let mut pwd = unsafe { std::mem::zeroed::<libc::passwd>() };
        let mut result: *mut libc::passwd = ptr::null_mut();
        let buf_size = unsafe { libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) };
        let buf_size = if buf_size > 0 {
            buf_size as usize
        } else {
            16 * 1024
        };
        let mut buf = vec![0u8; buf_size];

        let ret = unsafe {
            libc::getpwnam_r(
                c_username.as_ptr(),
                &mut pwd,
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };

        if ret != 0 {
            return Err(fdo::Error::Failed(format!(
                "failed to resolve user '{username}'"
            )));
        }
        if result.is_null() {
            return Err(fdo::Error::AccessDenied(format!(
                "unknown user '{username}'"
            )));
        }

        Ok(pwd.pw_uid)
    }

    async fn caller_uid(header: &Header<'_>) -> fdo::Result<u32> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;
        let conn = zbus::Connection::system()
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to connect to system bus: {e}")))?;
        let dbus = fdo::DBusProxy::new(&conn)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to create DBus proxy: {e}")))?;
        dbus.get_connection_unix_user(sender.to_owned().into())
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to get caller uid: {e}")))
    }

    async fn caller_pid(header: &Header<'_>) -> fdo::Result<u32> {
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;
        let conn = zbus::Connection::system()
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to connect to system bus: {e}")))?;
        let dbus = fdo::DBusProxy::new(&conn)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to create DBus proxy: {e}")))?;
        dbus.get_connection_unix_process_id(sender.to_owned().into())
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to get caller pid: {e}")))
    }

    fn environ_has_ssh_marker(environ: &[u8]) -> bool {
        environ.split(|b| *b == 0).any(|entry| {
            (entry.starts_with(b"SSH_CONNECTION=") && entry.len() > b"SSH_CONNECTION=".len())
                || (entry.starts_with(b"SSH_TTY=") && entry.len() > b"SSH_TTY=".len())
        })
    }

    fn process_is_ssh_session(pid: u32) -> Option<bool> {
        std::fs::read(format!("/proc/{pid}/environ"))
            .map(|env| Self::environ_has_ssh_marker(&env))
            .ok()
    }

    fn current_env_is_ssh_session() -> bool {
        std::env::var_os("SSH_CONNECTION").is_some_and(|value| !value.as_os_str().is_empty())
            || std::env::var_os("SSH_TTY").is_some_and(|value| !value.as_os_str().is_empty())
    }

    fn lid_state_is_closed(state: &str) -> bool {
        state.to_ascii_lowercase().contains("closed")
    }

    fn is_lid_closed_at(base: &std::path::Path) -> bool {
        let Ok(entries) = std::fs::read_dir(base) else {
            return false;
        };

        entries.filter_map(Result::ok).any(|entry| {
            std::fs::read_to_string(entry.path().join("state"))
                .map(|state| Self::lid_state_is_closed(&state))
                .unwrap_or(false)
        })
    }

    fn is_lid_closed() -> bool {
        Self::is_lid_closed_at(std::path::Path::new("/proc/acpi/button/lid"))
    }

    async fn ensure_auth_not_aborted(&self, header: &Header<'_>) -> fdo::Result<()> {
        let abort_if_ssh = *self.abort_if_ssh.lock().await;
        if abort_if_ssh {
            let caller_pid = Self::caller_pid(header).await.ok();
            let is_ssh = caller_pid
                .and_then(Self::process_is_ssh_session)
                .unwrap_or_else(Self::current_env_is_ssh_session);
            if is_ssh {
                warn!(caller_pid, "SSH session detected, aborting face auth");
                return Err(fdo::Error::Failed("SSH session detected".into()));
            }
        }

        let abort_if_lid_closed = *self.abort_if_lid_closed.lock().await;
        if abort_if_lid_closed && Self::is_lid_closed() {
            warn!("Laptop lid is closed, aborting face auth");
            return Err(fdo::Error::Failed("lid closed".into()));
        }

        Ok(())
    }

    async fn ensure_user_access(
        header: &Header<'_>,
        username: &str,
        action_id: &str,
    ) -> fdo::Result<()> {
        let caller_uid = Self::caller_uid(header).await?;
        let target_uid = Self::username_uid(username)?;
        if caller_uid == 0 || caller_uid == target_uid {
            return Ok(());
        }

        Self::ensure_authorized(header, action_id).await
    }

    fn signal_destination(sender: &str) -> fdo::Result<BusName<'static>> {
        BusName::try_from(sender.to_string())
            .map_err(|e| fdo::Error::Failed(format!("Invalid signal destination: {e}")))
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
            Ok((
                status,
                Some(FaceData {
                    embedding,
                    bbox: [x1, y1, x2, y2],
                    kpss: kpss.clone(),
                    yaw: res.yaw,
                    pitch: res.pitch,
                }),
            ))
        } else {
            Ok((status, None))
        }
    }

    async fn ensure_authorized(header: &Header<'_>, action_id: &str) -> fdo::Result<()> {
        let conn = zbus::Connection::system()
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to connect to system bus: {e}")))?;

        let authority = zbus_polkit::policykit1::AuthorityProxy::new(&conn)
            .await
            .map_err(|e| fdo::Error::Failed(format!("Failed to create polkit proxy: {e}")))?;

        let subject = zbus_polkit::policykit1::Subject::new_for_message_header(header)
            .map_err(|e| fdo::Error::Failed(format!("Failed to create polkit subject: {e}")))?;

        let details: HashMap<&str, &str> = HashMap::new();
        let flags = zbus_polkit::policykit1::CheckAuthorizationFlags::AllowUserInteraction.into();

        let result = authority
            .check_authorization(&subject, action_id, &details, flags, "")
            .await
            .map_err(|e| fdo::Error::Failed(format!("PolicyKit CheckAuthorization failed: {e}")))?;

        if !result.is_authorized {
            return Err(fdo::Error::AccessDenied(format!(
                "Authorization denied for action '{action_id}'"
            )));
        }

        Ok(())
    }

    async fn check_claim(&self, header: &Header<'_>) -> fdo::Result<ClaimState> {
        let sender = header
            .sender()
            .map(|s| s.to_string())
            .ok_or_else(|| fdo::Error::AccessDenied("Missing DBus sender".into()))?;

        let state = self.claim_state.lock().await;
        if let Some(claim) = &*state {
            if claim.sender == sender {
                return Ok(claim.clone());
            } else {
                return Err(fdo::Error::Failed(
                    "Daemon is claimed by another process".into(),
                ));
            }
        }
        Err(fdo::Error::Failed("Daemon is not claimed".into()))
    }

    fn has_pipewire_runtime(uid: u32) -> bool {
        std::path::Path::new(&format!("/run/user/{uid}/pipewire-0")).exists()
    }

    // PipeWire lives under /run/user/<uid> and we have to set XDG_RUNTIME_DIR to a uid that
    // actually has a session running. Priority: (1) caller==target with a runtime, (2) any
    // non-root caller with a runtime, (3) root calling on behalf of the active seat session,
    // (4) target's runtime, (5) the active seat session's runtime. Falls back to target if
    // nothing matches so the caller still gets a meaningful "no camera" error.
    async fn camera_runtime_uid(caller_uid: u32, target_uid: u32) -> u32 {
        if caller_uid == target_uid && Self::has_pipewire_runtime(target_uid) {
            return target_uid;
        }

        if caller_uid != 0 && Self::has_pipewire_runtime(caller_uid) {
            return caller_uid;
        }

        let active_uid = get_active_session_uid().await.ok();
        if let Some(active_uid) = active_uid
            && caller_uid == 0
            && Self::has_pipewire_runtime(active_uid)
        {
            return active_uid;
        }

        if Self::has_pipewire_runtime(target_uid) {
            return target_uid;
        }

        if let Some(active_uid) = active_uid
            && Self::has_pipewire_runtime(active_uid)
        {
            return active_uid;
        }

        warn!(
            target_uid,
            caller_uid, "No PipeWire runtime found for target, caller, or active session"
        );
        target_uid
    }

    fn cancel_active_tasks(&self) {
        if let Ok(mut cancel) = self.active_cancel.try_lock()
            && let Some(sender) = cancel.take()
        {
            let _ = sender.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AuthDaemon;

    #[test]
    fn ssh_marker_detection_requires_non_empty_values() {
        assert!(AuthDaemon::environ_has_ssh_marker(
            b"PATH=/usr/bin\0SSH_CONNECTION=1.2.3.4 1 5.6.7.8 22\0"
        ));
        assert!(AuthDaemon::environ_has_ssh_marker(
            b"SSH_TTY=/dev/pts/3\0USER=alice\0"
        ));
        assert!(!AuthDaemon::environ_has_ssh_marker(
            b"SSH_CONNECTION=\0SSH_TTY=\0"
        ));
        assert!(!AuthDaemon::environ_has_ssh_marker(b"USER=alice\0"));
    }

    #[test]
    fn lid_state_detection_is_case_insensitive() {
        assert!(AuthDaemon::lid_state_is_closed("state:      closed\n"));
        assert!(AuthDaemon::lid_state_is_closed("State: CLOSED\n"));
        assert!(!AuthDaemon::lid_state_is_closed("state:      open\n"));
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

pub fn set_pipewire_runtime_for_uid(uid: u32) {
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{uid}"));
    }
}

#[interface(name = "com.gundulabs.Gaze")]
impl AuthDaemon {
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

        let mut state = self.claim_state.lock().await;
        if let Some(existing) = &*state {
            if existing.sender == sender {
                return Ok(());
            }
            if caller_uid == 0 {
                self.cancel_active_tasks();
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

        let camera_uid = Self::camera_runtime_uid(caller_uid, target_uid).await;
        info!(
            sender = %sender,
            username = %username,
            target_uid,
            caller_uid,
            camera_uid,
            "Claimed daemon"
        );
        set_pipewire_runtime_for_uid(camera_uid);
        *state = Some(ClaimState {
            username,
            sender: sender.clone(),
        });
        drop(state);

        let claim_state = self.claim_state.clone();
        let active_cancel = self.active_cancel.clone();
        let sender_for_timeout = sender.clone();

        self.rt_handle.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(CLAIM_TIMEOUT_SECS)).await;
            let mut state = claim_state.lock().await;
            if let Some(claim) = &*state
                && claim.sender == sender_for_timeout
            {
                *state = None;
                let mut cancel = active_cancel.lock().await;
                if let Some(tx) = cancel.take() {
                    let _ = tx.send(());
                }
            }
        });

        let claim_state = self.claim_state.clone();
        let active_cancel = self.active_cancel.clone();
        let conn = conn.clone();
        let sender_for_watcher = sender.clone();

        self.rt_handle.spawn(async move {
            let Ok(dbus) = fdo::DBusProxy::new(&conn).await else {
                return;
            };

            let Ok(mut stream) = dbus.receive_name_owner_changed().await else {
                return;
            };

            while let Some(signal) = stream.next().await {
                if let Ok(args) = signal.args()
                    && args.name().as_str() == sender_for_watcher
                    && args.new_owner().is_none()
                {
                    info!(
                        sender = %sender_for_watcher,
                        "Sender vanished, auto-releasing claim"
                    );
                    let mut state = claim_state.lock().await;
                    if let Some(claim) = &*state
                        && claim.sender == sender_for_watcher
                    {
                        *state = None;
                        let mut cancel = active_cancel.lock().await;
                        if let Some(tx) = cancel.take() {
                            let _ = tx.send(());
                        }
                    }
                    break;
                }
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
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
        _face_name: String,
    ) -> fdo::Result<()> {
        let claim = self.check_claim(&header).await?;
        self.ensure_auth_not_aborted(&header).await?;
        let username = claim.username.clone();
        let signal_destination = Self::signal_destination(&claim.sender)?;
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
            let ctxt = SignalEmitter::new(&conn, path)
                .unwrap()
                .set_destination(signal_destination);


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

                if let Some(data) = embed_opt {
                    let embed = data.embedding;
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
        #[zbus(signal_context)] ctxt: SignalEmitter<'_>,
        #[zbus(header)] header: Header<'_>,
        face_name: String,
    ) -> fdo::Result<()> {
        let claim = self.check_claim(&header).await?;
        let username = claim.username.clone();
        let signal_destination = Self::signal_destination(&claim.sender)?;
        self.cancel_active_tasks();

        UserDatabase::validate_face_name(&face_name).map_err(Self::map_user_db_error)?;

        let (tx, mut rx) = oneshot::channel();
        *self.active_cancel.lock().await = Some(tx);

        let checker_arc = self.checker.clone();
        let recognizer_arc = self.recognizer.clone();
        let db_arc = self.db.clone();
        let camera_config = self.camera_config.lock().await.clone();

        let conn = ctxt.connection().clone();
        let path = ctxt.path().to_owned();

        self.rt_handle.spawn(async move {
            let ctxt = SignalEmitter::new(&conn, path)
                .unwrap()
                .set_destination(signal_destination);


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
            let mut last_kpss: Option<ndarray::Array3<f32>> = None;
            let mut stable_frames = 0;
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

                let (status, result_opt) = match Self::process_and_emit_status(&ctxt, &checker_arc, &recognizer_arc, &frame, &mut last_capture_status).await {
                    Ok(res) => res,
                    Err(_) => {
                        stable_frames = 0;
                        continue;
                    }
                };

                let Some(data) = result_opt else {
                    stable_frames = 0;
                    continue;
                };
                let embed = data.embedding;

                let is_stable = if let Some(ref _res) = last_capture_status
                    && status == CaptureStatus::Ready
                {
                    if let Some(prev_kps) = last_kpss.as_ref() {
                        let cur_kps = &data.kpss;
                        let delta: f32 = cur_kps.iter().zip(prev_kps.iter()).map(|(c, p)| (c - p).abs()).sum();
                        let [x1, _, x2, _] = data.bbox;
                        // Normalize landmark drift by face width so distance to camera doesn't
                        // change the bar; a small face is allowed less absolute jitter.
                        let face_w = x2 - x1;
                        let norm_delta = delta / face_w;
                        if norm_delta < 0.05 {
                            stable_frames += 1;
                        } else {
                            stable_frames = 0;
                        }

                        last_kpss = Some(cur_kps.clone());
                        stable_frames >= 3
                    } else {
                        last_kpss = Some(data.kpss.clone());
                        false
                    }
                } else {
                    stable_frames = 0;
                    false
                };

                let pose_matches = if let Some(ref _res) = last_capture_status
                    && status == CaptureStatus::Ready
                {
                    let yaw = data.yaw;
                    let pitch = data.pitch;

                    match prompt {
                        EnrollPrompt::LookStraight => yaw.abs() < 0.16 && (pitch - 0.48).abs() < 0.18,
                        EnrollPrompt::LookUp => pitch < 0.35,
                        EnrollPrompt::LookDown => pitch > 0.55,
                        EnrollPrompt::LookLeft => yaw < -0.15,
                        EnrollPrompt::LookRight => yaw > 0.15,
                        _ => false,
                    }
                } else {
                    false
                };

                macro_rules! send_enroll_status {
                    ($msg:expr, $rem:expr) => {
                        if Some($msg) != last_enroll_prompt || $rem > 0.0 {
                            let _ = Self::enroll_status(&ctxt, &face_name, current_step_idx as u32, max_steps, false, $msg, $rem).await;
                            last_enroll_prompt = Some($msg);
                        }
                    }
                }

                if status == CaptureStatus::Ready {
                    if is_stable && pose_matches {
                        captured_embeddings.push(embed);
                        let new_count = captured_embeddings.len() as u32;
                        stable_frames = 0;
                        last_enroll_prompt = None;
                        last_kpss = None;

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
                    } else {
                        send_enroll_status!(prompt, 0.0);
                    }
                } else {
                    stable_frames = 0;
                    send_enroll_status!(prompt, 0.0);
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

    async fn list_faces(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
    ) -> fdo::Result<Vec<(String, u32)>> {
        Self::ensure_user_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let db = self.db.lock().await;
        db.list_faces(&username).map_err(Self::map_user_db_error)
    }

    async fn delete_face(
        &self,
        #[zbus(header)] header: Header<'_>,
        username: String,
        face_name: String,
    ) -> fdo::Result<bool> {
        Self::ensure_user_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
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
        Self::ensure_user_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
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
        Self::ensure_user_access(&header, &username, POLKIT_ACTION_MANAGE_FACES).await?;
        let mut db = self.db.lock().await;
        db.clear_user(&username).map_err(Self::map_user_db_error)?;
        Ok(true)
    }

    async fn get_config(
        &self,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<HashMap<String, HashMap<String, OwnedValue>>> {
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_CONFIG).await?;
        let config = Config::load_from(CONFIG_PATH)
            .map_err(|e| fdo::Error::Failed(format!("Failed to load config: {e}")))?;
        Ok(config.to_map())
    }

    async fn set_config(
        &self,
        #[zbus(header)] header: Header<'_>,
        config: HashMap<String, HashMap<String, OwnedValue>>,
    ) -> fdo::Result<bool> {
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_CONFIG).await?;

        self.cancel_active_tasks();

        let new_config = Config::from_map(config)
            .map_err(|e| fdo::Error::Failed(format!("Invalid config: {e}")))?;

        let mut threshold = self.threshold.lock().await;
        *threshold = new_config.security.threshold();

        let mut camera_config = self.camera_config.lock().await;
        *camera_config = new_config.cameras.rgb.clone();

        let mut abort_if_ssh = self.abort_if_ssh.lock().await;
        *abort_if_ssh = new_config.auth.abort_if_ssh;

        let mut abort_if_lid_closed = self.abort_if_lid_closed.lock().await;
        *abort_if_lid_closed = new_config.auth.abort_if_lid_closed;

        let mut db = self.db.lock().await;
        db.set_max_templates(new_config.enrollment.max_templates as usize);

        let mut checker = self.checker.lock().await;
        let mut recognizer = self.recognizer.lock().await;

        let security = &new_config.security;
        info!(
            detector = security.detector(),
            recognizer = security.recognizer(),
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

        match gaze_core::detect::FaceDetector::new(det_path.to_str().unwrap()) {
            Ok(det) => *checker = FaceChecker::from_detector_with_config(det, &new_config),
            Err(e) => return Err(fdo::Error::Failed(format!("Failed to load detector: {e}"))),
        }

        match crate::recognize::FaceRecognizer::new(rec_path.to_str().unwrap()) {
            Ok(rec) => *recognizer = rec,
            Err(e) => {
                return Err(fdo::Error::Failed(format!(
                    "Failed to load recognizer: {e}"
                )));
            }
        }

        new_config
            .save_to(CONFIG_PATH)
            .map_err(|e| fdo::Error::Failed(format!("Failed to save config: {e}")))?;

        info!("Config reloaded successfully");
        Ok(true)
    }

    #[zbus(signal)]
    async fn verify_status(
        ctxt: &SignalEmitter<'_>,
        result: VerifyResult,
        faces: Vec<(String, f64, f64, bool, u32)>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn face_status(ctxt: &SignalEmitter<'_>, status: CaptureStatus) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn enroll_status(
        ctxt: &SignalEmitter<'_>,
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
        ctxt: &SignalEmitter<'_>,
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
