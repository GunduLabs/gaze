use futures::StreamExt;
use ndarray::Array1;
use opencv::core::Mat;
use std::collections::HashMap;
use std::ffi::CString;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, oneshot};
use tracing::{error, info, warn};
use zbus::names::BusName;
use zbus::{fdo, interface, message::Header, object_server::SignalEmitter};

use crate::align::{align_face, mat_to_rgb};
use crate::liveness::LivenessDetector;
use crate::recognize::FaceRecognizer;
use crate::users::{UserDatabase, UserDbError};
use gaze_core::camera::{Camera, CameraKind, resolve_configured_sources};
use gaze_core::config::Config;
use gaze_core::dbus::{CaptureStatus, EnrollPrompt, VerifyResult};
use gaze_core::detect::FaceDetector;
use gaze_core::face::{EnrollmentPoseStability, FaceChecker, Spectrum, enrollment_pose_matches};
use gaze_core::ir::led::IrLed;

const CONFIG_PATH: &str = "/etc/gaze/config.toml";
const POLKIT_ACTION_MANAGE_FACES: &str = "com.gundulabs.gaze.manage-faces";
const POLKIT_ACTION_MANAGE_CONFIG: &str = "com.gundulabs.gaze.manage-config";
const POLKIT_ACTION_MANAGE_GDM_PROFILE: &str = "com.gundulabs.gaze.manage-gdm-profile";
const GDM_DCONF_OVERRIDE_PATH: &str = "/etc/dconf/db/gdm.d/99-gaze";
const GDM_DCONF_OVERRIDE_CONTENT: &str =
    "[org/gnome/shell/extensions/gaze]\nenable-face-authentication=true\n";
const CLAIM_TIMEOUT_SECS: u64 = 300;
const VERIFY_TOO_DARK_TIMEOUT: Duration = Duration::from_secs(1);
const SSH_PROC_CHAIN_MAX_DEPTH: usize = 16;

#[derive(Clone)]
pub struct ClaimState {
    pub username: String,
    pub sender: String,
    pub epoch: u64,
}

static CLAIM_EPOCH: AtomicU64 = AtomicU64::new(0);

fn claim_has_epoch(state: &Option<ClaimState>, epoch: u64) -> bool {
    matches!(state, Some(claim) if claim.epoch == epoch)
}

pub struct FaceData {
    pub embedding: Array1<f32>,
    pub liveness_frame: Option<Mat>,
    /// Unpadded frame size; `liveness_frame` and `bbox` use square-padded coordinates.
    pub frame_size: (u32, u32),
    pub bbox: [f32; 4],
    pub kpss: ndarray::Array3<f32>,
    pub yaw: f32,
    pub pitch: f32,
}

struct EmitterGuard {
    led: Option<IrLed>,
}

impl EmitterGuard {
    fn engage(kind: &CameraKind, enabled: bool) -> Self {
        let led = match kind {
            CameraKind::Ir { node, .. } if enabled => match IrLed::for_path(node) {
                Some(led) => {
                    if let Err(e) = led.set(true) {
                        warn!("IR emitter activate failed: {e}");
                    }
                    Some(led)
                }
                None => {
                    warn!("no IR emitter profile for {node}; continuing without illumination");
                    None
                }
            },
            _ => None,
        };
        Self { led }
    }
}

impl Drop for EmitterGuard {
    fn drop(&mut self) {
        if let Some(led) = &self.led
            && let Err(e) = led.set(false)
        {
            warn!("IR emitter deactivate failed: {e}");
        }
    }
}

fn eyes_from_kpss(kpss: &ndarray::Array3<f32>) -> Option<[(f32, f32); 5]> {
    let shape = kpss.shape();
    if shape[0] < 1 || shape[1] < 5 || shape[2] < 2 {
        return None;
    }
    let mut pts = [(0.0f32, 0.0f32); 5];
    for (i, p) in pts.iter_mut().enumerate() {
        *p = (kpss[[0, i, 0]], kpss[[0, i, 1]]);
    }
    Some(pts)
}

pub struct AuthDaemon {
    pub detector: Arc<std::sync::Mutex<FaceDetector>>,
    pub recognizer_rgb: Arc<Mutex<FaceRecognizer>>,
    pub recognizer_ir: Arc<Mutex<FaceRecognizer>>,
    pub liveness: Arc<Mutex<Option<LivenessDetector>>>,
    pub db: Arc<Mutex<UserDatabase>>,
    pub threshold: Arc<Mutex<f32>>,
    pub rgb_device: Arc<Mutex<String>>,
    pub ir_device: Arc<Mutex<String>>,
    pub ir_node: Arc<Mutex<String>>,
    pub emitter_enabled: Arc<Mutex<bool>>,
    pub liveness_config: Arc<Mutex<gaze_core::config::LivenessConfig>>,
    pub abort_if_ssh: Arc<Mutex<bool>>,
    pub abort_if_lid_closed: Arc<Mutex<bool>>,
    pub claim_state: Arc<Mutex<Option<ClaimState>>>,
    pub active_cancel: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    pub active_extensions: Arc<Mutex<std::collections::HashMap<u32, bool>>>,
    pub resume_pending: Arc<AtomicBool>,
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

    fn may_query_extension(caller_uid: u32, target_uid: u32) -> bool {
        caller_uid == 0 || caller_uid == target_uid
    }

    async fn emit_effective_face_status(
        ctxt: &SignalEmitter<'_>,
        last_emitted_status: &mut Option<CaptureStatus>,
        rgb_status: CaptureStatus,
        ir_status: CaptureStatus,
    ) {
        let effective_status = if rgb_status.priority() >= ir_status.priority() {
            rgb_status
        } else {
            ir_status
        };
        if last_emitted_status.as_ref() != Some(&effective_status) {
            let _ = Self::face_status(ctxt, effective_status).await;
            *last_emitted_status = Some(effective_status);
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

    fn read_ppid_at(base: &std::path::Path, pid: u32) -> Option<u32> {
        let stat = std::fs::read_to_string(base.join(pid.to_string()).join("stat")).ok()?;
        let after_comm = stat.rsplit_once(')')?.1;
        let mut fields = after_comm.split_whitespace();
        let _state = fields.next()?;
        fields.next()?.parse::<u32>().ok()
    }

    fn proc_is_sshd_at(base: &std::path::Path, pid: u32) -> bool {
        std::fs::read_to_string(base.join(pid.to_string()).join("comm"))
            .map(|comm| {
                let comm = comm.trim();
                comm == "sshd" || comm == "sshd-session"
            })
            .unwrap_or(false)
    }

    fn proc_environ_is_ssh_at(base: &std::path::Path, pid: u32) -> bool {
        std::fs::read(base.join(pid.to_string()).join("environ"))
            .map(|env| Self::environ_has_ssh_marker(&env))
            .unwrap_or(false)
    }
    fn process_chain_is_ssh_at(base: &std::path::Path, pid: u32) -> bool {
        let mut current = pid;
        for _ in 0..SSH_PROC_CHAIN_MAX_DEPTH {
            if Self::proc_environ_is_ssh_at(base, current) || Self::proc_is_sshd_at(base, current) {
                return true;
            }
            match Self::read_ppid_at(base, current) {
                Some(ppid) if ppid != 0 && ppid != current => current = ppid,
                _ => break,
            }
        }
        false
    }

    fn process_is_ssh_session(pid: u32) -> bool {
        Self::process_chain_is_ssh_at(std::path::Path::new("/proc"), pid)
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

    fn upower_lid_closed(present: bool, closed: bool) -> bool {
        present && closed
    }
    async fn lid_is_closed_via_upower() -> Option<bool> {
        let conn = zbus::Connection::system().await.ok()?;
        let proxy = zbus::Proxy::new(
            &conn,
            "org.freedesktop.UPower",
            "/org/freedesktop/UPower",
            "org.freedesktop.UPower",
        )
        .await
        .ok()?;
        let present: bool = proxy.get_property("LidIsPresent").await.ok()?;
        let closed: bool = proxy.get_property("LidIsClosed").await.ok()?;
        Some(Self::upower_lid_closed(present, closed))
    }

    async fn is_lid_closed() -> bool {
        if let Some(closed) = Self::lid_is_closed_via_upower().await {
            return closed;
        }
        Self::is_lid_closed_at(std::path::Path::new("/proc/acpi/button/lid"))
    }

    async fn ensure_auth_not_aborted(&self, header: &Header<'_>) -> fdo::Result<()> {
        let abort_if_ssh = *self.abort_if_ssh.lock().await;
        if abort_if_ssh {
            let caller_pid = Self::caller_pid(header).await.ok();
            let is_ssh = match caller_pid {
                Some(pid) => Self::process_is_ssh_session(pid),
                None => Self::current_env_is_ssh_session(),
            };
            if is_ssh {
                warn!(caller_pid, "SSH session detected, aborting face auth");
                return Err(fdo::Error::Failed("SSH session detected".into()));
            }
        }

        let abort_if_lid_closed = *self.abort_if_lid_closed.lock().await;
        if abort_if_lid_closed && Self::is_lid_closed().await {
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

    // The GDM greeter asks which login users have faces and cannot answer an
    // interactive polkit challenge. `active` is (uid, is_greeter) for the seat.
    fn user_query_allowed(caller_uid: u32, target_uid: u32, active: Option<(u32, bool)>) -> bool {
        if caller_uid == 0 || caller_uid == target_uid {
            return true;
        }
        matches!(active, Some((uid, true)) if uid == caller_uid)
    }

    async fn ensure_user_query_access(
        header: &Header<'_>,
        username: &str,
        action_id: &str,
    ) -> fdo::Result<()> {
        let caller_uid = Self::caller_uid(header).await?;
        let target_uid = Self::username_uid(username)?;
        let active = match gaze_core::dbus::get_active_session_uid_and_class().await {
            Ok((uid, class)) => Some((uid, class == "greeter")),
            Err(_) => None,
        };
        if Self::user_query_allowed(caller_uid, target_uid, active) {
            return Ok(());
        }

        Self::ensure_authorized(header, action_id).await
    }

    fn signal_destination(sender: &str) -> fdo::Result<BusName<'static>> {
        BusName::try_from(sender.to_string())
            .map_err(|e| fdo::Error::Failed(format!("Invalid signal destination: {e}")))
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

    // Bind capture to the target's own session; a bystander's camera must never authenticate
    // another user. `active` is (uid, is_greeter, has_pipewire) for the active seat.
    fn resolve_camera_uid(
        caller_uid: u32,
        target_uid: u32,
        target_has_pipewire: bool,
        caller_has_pipewire: bool,
        active: Option<(u32, bool, bool)>,
    ) -> Option<u32> {
        // An active greeter holds the seat's camera ACL, so it outranks the target's leftover PipeWire socket.
        if caller_uid == 0
            && let Some((active_uid, true, true)) = active
        {
            return Some(active_uid);
        }
        if target_has_pipewire {
            return Some(target_uid);
        }
        if caller_uid != 0 {
            return caller_has_pipewire.then_some(caller_uid);
        }
        None
    }

    async fn camera_runtime_uid(caller_uid: u32, target_uid: u32) -> Option<u32> {
        let active = match gaze_core::dbus::get_active_session_uid_and_class().await {
            Ok((uid, class)) => Some((uid, class == "greeter", Self::has_pipewire_runtime(uid))),
            Err(_) => None,
        };
        Self::resolve_camera_uid(
            caller_uid,
            target_uid,
            Self::has_pipewire_runtime(target_uid),
            Self::has_pipewire_runtime(caller_uid),
            active,
        )
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
    use super::{
        AuthDaemon, ClaimState, auth_streams, claim_has_epoch, eyes_from_kpss, hybrid_auth_passed,
    };
    use gaze_core::dbus::CaptureStatus;

    #[test]
    fn stale_claim_epoch_does_not_match_reclaimed_state() {
        let state = Some(ClaimState {
            username: "alice".to_string(),
            sender: ":1.42".to_string(),
            epoch: 2,
        });

        assert!(!claim_has_epoch(&state, 1));
        assert!(claim_has_epoch(&state, 2));
    }

    #[test]
    fn eyes_from_kpss_extracts_first_face_landmarks() {
        let kpss = ndarray::Array3::from_shape_fn((1, 5, 2), |(_, i, c)| (i * 2 + c) as f32);
        let eyes = eyes_from_kpss(&kpss).expect("valid kpss shape");
        assert_eq!(eyes[0], (0.0, 1.0));
        assert_eq!(eyes[1], (2.0, 3.0));
    }

    #[test]
    fn eyes_from_kpss_rejects_malformed_shapes() {
        assert!(eyes_from_kpss(&ndarray::Array3::zeros((0, 5, 2))).is_none());
        assert!(eyes_from_kpss(&ndarray::Array3::zeros((1, 3, 2))).is_none());
        assert!(eyes_from_kpss(&ndarray::Array3::zeros((1, 5, 1))).is_none());
    }

    #[test]
    fn liveness_crop_excludes_square_padding_bars() {
        use super::{FaceData, crop_liveness_face};
        use opencv::core::{CV_8UC3, Mat, Scalar};

        let frame = Mat::new_rows_cols_with_default(480, 640, CV_8UC3, Scalar::all(255.0)).unwrap();
        let padded = gaze_core::detect::FaceDetector::pad_to_square(&frame).unwrap();

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
    fn emitter_guard_is_inert_for_rgb_and_when_disabled() {
        use super::EmitterGuard;
        use gaze_core::camera::CameraKind;

        assert!(
            EmitterGuard::engage(
                &CameraKind::Rgb {
                    source: "primary".to_string()
                },
                true
            )
            .led
            .is_none()
        );
        assert!(
            EmitterGuard::engage(
                &CameraKind::Ir {
                    source: "primary".to_string(),
                    node: "/dev/null".to_string()
                },
                false
            )
            .led
            .is_none()
        );
    }

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

    #[test]
    fn upower_lid_closed_requires_present_and_closed() {
        assert!(AuthDaemon::upower_lid_closed(true, true));
        // A machine without a lid (e.g. a desktop) is never "closed".
        assert!(!AuthDaemon::upower_lid_closed(true, false));
        assert!(!AuthDaemon::upower_lid_closed(false, true));
        assert!(!AuthDaemon::upower_lid_closed(false, false));
    }

    struct FakeProc {
        root: std::path::PathBuf,
    }

    impl FakeProc {
        fn new(name: &str) -> Self {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "gaze-proc-test-{}-{}-{name}",
                std::process::id(),
                unique
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn add(&self, pid: u32, ppid: u32, comm: &str, environ: &[u8]) {
            let dir = self.root.join(pid.to_string());
            // Embed parens/spaces in comm to exercise the stat parser.
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("stat"),
                format!("{pid} ({comm}) S {ppid} 1 1 0 -1 0\n"),
            )
            .unwrap();
            std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
            std::fs::write(dir.join("environ"), environ).unwrap();
        }

        fn root(&self) -> &std::path::Path {
            &self.root
        }
    }

    impl Drop for FakeProc {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn read_ppid_parses_stat_with_parenthesised_comm() {
        let proc = FakeProc::new("ppid");
        proc.add(42, 7, "weird (name)", b"");
        assert_eq!(AuthDaemon::read_ppid_at(proc.root(), 42), Some(7));
        assert_eq!(AuthDaemon::read_ppid_at(proc.root(), 999), None);
    }

    #[test]
    fn ssh_detected_via_ancestor_environ_marker() {
        let proc = FakeProc::new("ancestor-env");
        proc.add(1000, 900, "sshd", b"SSH_CONNECTION=1.2.3.4 5 6.7.8.9 22\0");
        proc.add(1001, 1000, "sudo", b"USER=alice\0");
        proc.add(1002, 1001, "unix_chkpwd", b"USER=alice\0");

        assert!(AuthDaemon::process_chain_is_ssh_at(proc.root(), 1002));
    }

    #[test]
    fn ssh_detected_via_ancestor_comm_when_environ_is_bare() {
        let proc = FakeProc::new("ancestor-comm");
        proc.add(2000, 1, "sshd-session", b"PATH=/usr/bin\0");
        proc.add(2001, 2000, "bash", b"PATH=/usr/bin\0");
        proc.add(2002, 2001, "sudo", b"PATH=/usr/bin\0");

        assert!(AuthDaemon::process_chain_is_ssh_at(proc.root(), 2002));
    }

    #[test]
    fn local_session_chain_is_not_flagged_as_ssh() {
        let proc = FakeProc::new("local");
        proc.add(3000, 1, "systemd", b"PATH=/usr/bin\0");
        proc.add(3001, 3000, "gdm-session-wor", b"PATH=/usr/bin\0");
        proc.add(3002, 3001, "sudo", b"USER=alice\0");

        assert!(!AuthDaemon::process_chain_is_ssh_at(proc.root(), 3002));
    }

    #[test]
    fn process_chain_walk_terminates_on_self_referential_ppid() {
        let proc = FakeProc::new("cycle");
        proc.add(4000, 4000, "bash", b"USER=alice\0");
        assert!(!AuthDaemon::process_chain_is_ssh_at(proc.root(), 4000));
    }

    #[test]
    fn camera_uses_target_own_session_when_logged_in() {
        // su victim while victim is logged in -> victim's own camera, not the attacker's.
        let attacker_active = Some((1000, false, true));
        assert_eq!(
            AuthDaemon::resolve_camera_uid(0, 1001, true, false, attacker_active),
            Some(1001)
        );
    }

    #[test]
    fn camera_refuses_bystander_session_for_root_caller() {
        // su victim while victim has no session; the active seat is a regular user (attacker).
        let attacker_active = Some((1000, false, true));
        assert_eq!(
            AuthDaemon::resolve_camera_uid(0, 1001, false, false, attacker_active),
            None
        );
        // No active session info at all -> also refuse.
        assert_eq!(
            AuthDaemon::resolve_camera_uid(0, 1001, false, false, None),
            None
        );
    }

    #[test]
    fn user_queries_allow_root_self_and_active_greeter_only() {
        assert!(AuthDaemon::user_query_allowed(0, 1000, None));
        assert!(AuthDaemon::user_query_allowed(1000, 1000, None));
        // Active greeter may ask about any login user.
        assert!(AuthDaemon::user_query_allowed(42, 1000, Some((42, true))));
        // Non-greeter or inactive callers still need polkit.
        assert!(!AuthDaemon::user_query_allowed(42, 1000, Some((42, false))));
        assert!(!AuthDaemon::user_query_allowed(
            42,
            1000,
            Some((1000, true))
        ));
        assert!(!AuthDaemon::user_query_allowed(42, 1000, None));
    }

    #[test]
    fn camera_allows_login_greeter_for_root_caller() {
        // GDM login: target has no session yet, active seat is the greeter.
        let greeter_active = Some((42, true, true));
        assert_eq!(
            AuthDaemon::resolve_camera_uid(0, 1001, false, false, greeter_active),
            Some(42)
        );
        // Greeter without a usable camera runtime -> refuse.
        assert_eq!(
            AuthDaemon::resolve_camera_uid(0, 1001, false, false, Some((42, true, false))),
            None
        );
    }

    #[test]
    fn camera_prefers_active_greeter_over_target_leftover_runtime() {
        // GDM login while the target's runtime lingers: the greeter owns the seat camera (issue #193).
        let greeter_active = Some((42, true, true));
        assert_eq!(
            AuthDaemon::resolve_camera_uid(0, 1001, true, false, greeter_active),
            Some(42)
        );
        // Greeter active but without PipeWire -> fall back to the target's runtime.
        assert_eq!(
            AuthDaemon::resolve_camera_uid(0, 1001, true, false, Some((42, true, false))),
            Some(1001)
        );
    }

    #[test]
    fn camera_uses_caller_session_for_polkit_approved_caller() {
        // Admin (non-root) acting for another user after a polkit check uses their own camera.
        assert_eq!(
            AuthDaemon::resolve_camera_uid(1000, 1001, false, true, Some((1000, false, true))),
            Some(1000)
        );
        // ...but refuse if even the caller has no camera session.
        assert_eq!(
            AuthDaemon::resolve_camera_uid(1000, 1001, false, false, None),
            None
        );
    }

    #[test]
    fn extension_state_is_visible_only_to_root_or_the_target_user() {
        assert!(AuthDaemon::may_query_extension(0, 1000));
        assert!(AuthDaemon::may_query_extension(1000, 1000));
        assert!(!AuthDaemon::may_query_extension(1001, 1000));
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
        assert!(hybrid_auth_passed(
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

pub use gaze_core::dbus::get_active_session_uid;

pub fn set_pipewire_runtime_for_uid(uid: u32) {
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", format!("/run/user/{uid}"));
    }
}

async fn prepare_for_sleep_stream(conn: &zbus::Connection) -> zbus::Result<zbus::MessageStream> {
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .interface("org.freedesktop.login1.Manager")?
        .member("PrepareForSleep")?
        .path("/org/freedesktop/login1")?
        .build();
    zbus::MessageStream::for_match_rule(rule, conn, None).await
}

pub async fn watch_resume(conn: zbus::Connection, resume_pending: Arc<AtomicBool>) {
    let mut stream = match prepare_for_sleep_stream(&conn).await {
        Ok(stream) => stream,
        Err(e) => {
            warn!("Failed to subscribe to PrepareForSleep, resume handling disabled: {e}");
            return;
        }
    };

    while let Some(Ok(msg)) = stream.next().await {
        if let Ok(false) = msg.body().deserialize::<bool>() {
            resume_pending.store(true, Ordering::SeqCst);
        }
    }
}

enum VerifyMsg {
    Status(Spectrum, CaptureStatus, Option<ndarray::Array1<f32>>),
    Success(Spectrum, ndarray::Array1<f32>),
    Error(String),
}

fn hybrid_auth_passed(
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
            _ => {
                if !rgb_attempted {
                    rgb_success && ir_success
                } else if matches!(rgb_status, CaptureStatus::TooDark | CaptureStatus::NoFace) {
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

fn auth_streams(
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

fn process_frame_sync(
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

// Strip the square padding first: its black bars read as a replay bezel to the anti-spoof model.
fn crop_liveness_face(data: &FaceData) -> anyhow::Result<image::RgbImage> {
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

fn build_hybrid_scores(
    db: &UserDatabase,
    username: &str,
    threshold: f32,
    rgb_embed: Option<&ndarray::Array1<f32>>,
    ir_embed: Option<&ndarray::Array1<f32>>,
) -> Vec<(String, f64, f64, bool, f64, f64, bool)> {
    let rgb_scores = rgb_embed.and_then(|embed| {
        db.match_faces(username, embed, threshold, Spectrum::Rgb)
            .ok()
    });
    let ir_scores = ir_embed.and_then(|embed| {
        db.match_faces(username, embed, threshold, Spectrum::Ir)
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

        let Some(camera_uid) = Self::camera_runtime_uid(caller_uid, target_uid).await else {
            return Err(fdo::Error::AccessDenied(
                "refusing face auth: no camera belongs to the target user's session".into(),
            ));
        };

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

        info!(
            sender = %sender,
            username = %username,
            target_uid,
            caller_uid,
            camera_uid,
            "Claimed daemon"
        );
        set_pipewire_runtime_for_uid(camera_uid);
        let epoch = CLAIM_EPOCH.fetch_add(1, Ordering::Relaxed);
        *state = Some(ClaimState {
            username,
            sender: sender.clone(),
            epoch,
        });
        drop(state);

        let claim_state = self.claim_state.clone();
        let active_cancel = self.active_cancel.clone();

        self.rt_handle.spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(CLAIM_TIMEOUT_SECS)).await;
            let mut state = claim_state.lock().await;
            if claim_has_epoch(&state, epoch) {
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

        if self.resume_pending.swap(false, Ordering::SeqCst) {
            let grace = Duration::from_millis(
                Config::load_from(CONFIG_PATH)
                    .map(|c| c.auth.resume_grace_ms)
                    .unwrap_or(0),
            );
            if !grace.is_zero() {
                info!(
                    ?grace,
                    "Resumed from suspend, delaying face auth for display"
                );
                tokio::time::sleep(grace).await;
            }
        }

        let username = claim.username.clone();
        let signal_destination = Self::signal_destination(&claim.sender)?;
        self.cancel_active_tasks();

        let (tx, mut rx) = oneshot::channel();
        *self.active_cancel.lock().await = Some(tx);

        let detector_arc = self.detector.clone();
        let recognizer_rgb_arc = self.recognizer_rgb.clone();
        let recognizer_ir_arc = self.recognizer_ir.clone();
        let liveness_arc = self.liveness.clone();
        let db_arc = self.db.clone();
        let threshold_arc = self.threshold.clone();

        let config = Config::load_from(CONFIG_PATH).unwrap_or_default();
        let rgb_device = self.rgb_device.lock().await.clone();
        let ir_device = self.ir_device.lock().await.clone();
        let ir_node = self.ir_node.lock().await.clone();
        let emitter_enabled = *self.emitter_enabled.lock().await;
        let liveness_cfg = self.liveness_config.lock().await.clone();
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

            info!(
                liveness_enabled = liveness_cfg.enabled,
                liveness_threshold = liveness_cfg.threshold,
                run_rgb = run_rgb,
                run_ir = run_ir,
                "VerifyStart: sensing faces for user {}",
                username
            );

            let (result_tx, mut result_rx) = tokio::sync::mpsc::channel::<VerifyMsg>(10);
            let stop_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));

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
                let threshold_arc = threshold_arc.clone();
                let liveness_enabled = liveness_cfg.enabled;
                let liveness_threshold = liveness_cfg.threshold;
                let rgb_device_clone = rgb_device.clone();

                rgb_thread = Some(std::thread::spawn(move || {
                    let mut cam = match Camera::open(&rgb_device_clone) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(VerifyMsg::Error(format!("RGB Camera open error: {e}")));
                            return;
                        }
                    };
                    tracing::debug!("RGB camera opened successfully at: {}", rgb_device_clone);

                    let mut checker = FaceChecker::new(detector_arc, &config_clone, Spectrum::Rgb, false);
                    let mut live_scores: Vec<f32> = Vec::new();
                    let mut landmark_seq: Vec<[(f32, f32); 5]> = Vec::new();

                    for frame in &mut cam {
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }

                        let (status, embed_opt) = {
                            let mut recognizer = recognizer_rgb_arc.blocking_lock();
                            match process_frame_sync(&mut checker, &mut recognizer, &frame, liveness_enabled) {
                                Ok(res) => res,
                                Err(_) => (CaptureStatus::NoFace, None),
                            }
                        };
                        tracing::debug!("Processed RGB frame: status={:?}, embedding_extracted={}", status, embed_opt.is_some());

                        let latest_embed = embed_opt.as_ref().map(|d| d.embedding.clone());
                        let _ = tx.try_send(VerifyMsg::Status(Spectrum::Rgb, status, latest_embed));

                        if status == CaptureStatus::Usable && let Some(data) = embed_opt {
                            let threshold = *threshold_arc.blocking_lock();
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
                                        error!("Liveness is enabled but detector is unavailable");
                                        return;
                                    };
                                    let live_score = match detector.live_score(&liveness_face) {
                                        Ok(score) => score,
                                        Err(e) => {
                                            error!("Liveness inference failed: {e}");
                                            return;
                                        }
                                    };
                                    drop(live_guard);
                                    live_scores.push(live_score);

                                    let model_pass = crate::liveness::liveness_passes(&live_scores, liveness_threshold as f32);
                                    let motion = crate::liveness::eye_motion_is_live(&landmark_seq, None);
                                    let confirmed_static = motion.pairs >= 1 && !motion.live;
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

                    if !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                        let _ = tx.blocking_send(VerifyMsg::Error(
                            "RGB camera stream stopped unexpectedly".into(),
                        ));
                    }
                }));
            }

            let mut ir_thread = None;
                // If RGB is also running, introduce a short delay to ensure RGB gets the head start on PipeWire/USB resource access.
            if run_ir {
                if run_rgb {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }

                let stop_clone = stop_flag.clone();
                let tx = result_tx.clone();
                let detector_arc = detector_arc.clone();
                let config_clone = config.clone();
                let recognizer_ir_arc = recognizer_ir_arc.clone();
                let db_arc = db_arc.clone();
                let username_clone = username.clone();
                let threshold_arc = threshold_arc.clone();
                let liveness_enabled = liveness_cfg.enabled;
                let ir_device_clone = ir_device.clone();
                let ir_node_clone = ir_node.clone();
                let emitter_enabled = emitter_enabled;

                ir_thread = Some(std::thread::spawn(move || {
                    let _emitter = EmitterGuard::engage(
                        &CameraKind::Ir { source: ir_device_clone.clone(), node: ir_node_clone.clone() },
                        emitter_enabled
                    );

                    let mut cam = match Camera::open_ir(&ir_device_clone) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(VerifyMsg::Error(format!("IR Camera open error: {e}")));
                            return;
                        }
                    };
                    tracing::debug!("IR camera opened successfully at: {}", ir_device_clone);

                    let mut checker = FaceChecker::new(detector_arc, &config_clone, Spectrum::Ir, false);
                    let mut landmark_seq: Vec<[(f32, f32); 5]> = Vec::new();

                    for frame in &mut cam {
                        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }

                        let (status, embed_opt) = {
                            let mut recognizer = recognizer_ir_arc.blocking_lock();
                            match process_frame_sync(&mut checker, &mut recognizer, &frame, false) {
                                Ok(res) => res,
                                Err(_) => (CaptureStatus::NoFace, None),
                            }
                        };
                        tracing::debug!("Processed IR frame: status={:?}, embedding_extracted={}", status, embed_opt.is_some());

                        let latest_embed = embed_opt.as_ref().map(|d| d.embedding.clone());
                        let _ = tx.try_send(VerifyMsg::Status(Spectrum::Ir, status, latest_embed));

                        if status == CaptureStatus::Usable && let Some(data) = embed_opt {
                            let threshold = *threshold_arc.blocking_lock();
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
                                let mut liveness_passed = true;
                                if liveness_enabled {
                                    if let Some(eyes) = eyes_from_kpss(&data.kpss) {
                                        landmark_seq.push(eyes);
                                    }
                                    let motion = crate::liveness::eye_motion_is_live(&landmark_seq, None);
                                    liveness_passed = motion.pairs >= 1 && motion.live;

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
                        let _ = tx.blocking_send(VerifyMsg::Error(
                            "IR camera stream stopped unexpectedly".into(),
                        ));
                    }
                }));
            }

            let mut last_emitted_status: Option<CaptureStatus> = None;
            let mut rgb_status = CaptureStatus::Unused;
            let mut ir_status = CaptureStatus::Unused;
            let mut rgb_attempted = false;
            let mut dark_since: Option<Instant> = None;
            let mut frames_seen: u32 = 0;

            let mut rgb_success_embed = None;
            let mut ir_success_embed = None;
            let mut rgb_latest_embed = None;
            let mut ir_latest_embed = None;

            macro_rules! emit_verify_with_scores {
                ($result:expr) => {{
                    let threshold = *threshold_arc.lock().await;
                    let db = db_arc.lock().await;
                    let final_scores = build_hybrid_scores(
                        &db,
                        &username,
                        threshold,
                        rgb_success_embed.as_ref().or(rgb_latest_embed.as_ref()),
                        ir_success_embed.as_ref().or(ir_latest_embed.as_ref()),
                    );
                    drop(db);
                    let _ = Self::verify_status(&ctxt, $result, final_scores, rgb_status, ir_status).await;
                }};
            }

            macro_rules! finish_if_auth_passed {
                () => {{
                    if hybrid_auth_passed(
                        config.security.hybrid_policy(),
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
                        let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), rgb_status, ir_status).await;
                        break;
                    }
                    msg_opt = result_rx.recv() => {
                        let Some(msg) = msg_opt else { break };
                        match msg {
                            VerifyMsg::Status(spectrum, status, embed_opt) => {
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

                                if has_face {
                                    frames_seen += 1;
                                    if frames_seen >= liveness_cfg.max_frames {
                                        info!("VerifyStart: liveness gate timed out");
                                        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                        emit_verify_with_scores!(VerifyResult::VerifyNoMatch);
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
                                            "VerifyStart: giving up after {}s of dark frames",
                                            VERIFY_TOO_DARK_TIMEOUT.as_secs()
                                        );
                                        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                        let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), rgb_status, ir_status).await;
                                        break;
                                    }
                                } else {
                                    dark_since = None;
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
                            VerifyMsg::Error(e) => {
                                error!("VerifyStart loop error: {e}");
                                stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                let _ = Self::verify_status(&ctxt, VerifyResult::VerifyNoMatch, Vec::new(), rgb_status, ir_status).await;
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

        let detector_arc = self.detector.clone();
        let recognizer_rgb_arc = self.recognizer_rgb.clone();
        let recognizer_ir_arc = self.recognizer_ir.clone();
        let db_arc = self.db.clone();

        let config = Config::load_from(CONFIG_PATH).unwrap_or_default();
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

                rgb_thread = Some(std::thread::spawn(move || {
                    let mut cam = match Camera::open(&rgb_device_clone) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(EnrollMsg::Error(format!("RGB Camera open error: {e}")));
                            return;
                        }
                    };

                    let mut checker = FaceChecker::new(detector_arc, &config_clone, Spectrum::Rgb, true);
                    let mut last_processed_step = 999;
                    let mut captured_for_step = false;
                    let mut pose_stability = EnrollmentPoseStability::default();
                    let mut pose_baseline = None;

                    for frame in &mut cam {
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

                ir_thread = Some(std::thread::spawn(move || {
                    let _emitter = EmitterGuard::engage(
                        &CameraKind::Ir { source: ir_device_clone.clone(), node: ir_node_clone.clone() },
                        emitter_enabled
                    );

                    let mut cam = match Camera::open_ir(&ir_device_clone) {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = tx.blocking_send(EnrollMsg::Error(format!("IR Camera open error: {e}")));
                            return;
                        }
                    };

                    let mut checker = FaceChecker::new(detector_arc, &config_clone, Spectrum::Ir, true);
                    let mut last_processed_step = 999;
                    let mut captured_for_step = false;
                    let mut pose_stability = EnrollmentPoseStability::default();
                    let mut pose_baseline = None;

                    for frame in &mut cam {
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
                            let is_stable = if run_rgb {
                                rgb_captured_for_step_clone.load(std::sync::atomic::Ordering::Relaxed)
                            } else {
                                pose_stability.update(prompt, data.yaw, data.pitch)
                            };

                            let pose_matches = if run_rgb {
                                rgb_captured_for_step_clone.load(std::sync::atomic::Ordering::Relaxed)
                            } else {
                                enrollment_pose_matches(
                                    prompt,
                                    data.yaw,
                                    data.pitch,
                                    pose_baseline,
                                )
                            };

                            if is_stable && pose_matches {
                                if !run_rgb && prompt == EnrollPrompt::LookStraight {
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

            while completed_steps < max_steps as usize {
                let prompt = prompts[completed_steps];
                if last_sent_prompt != Some(prompt) {
                    let _ = Self::enroll_status(&ctxt, &face_name, completed_steps as u32, max_steps, false, prompt, 0.0).await;
                    last_sent_prompt = Some(prompt);
                }

                tokio::select! {
                    _ = &mut rx => {
                        info!("EnrollStart: cancelled");
                        let _ = Self::enroll_status(&ctxt, &face_name, 0, max_steps, true, EnrollPrompt::Cancelled, -1.0).await;
                        stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    msg_opt = enroll_rx.recv() => {
                        let Some(msg) = msg_opt else { break };
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
                                let _ = Self::enroll_status(&ctxt, &face_name, max_steps, max_steps, true, EnrollPrompt::DbFailed, -1.0).await;
                                stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                                return;
                            }
                        }
                    }
                }
            }

            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
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
        self.cancel_active_tasks();
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
        Ok(Self::camera_runtime_uid(caller_uid, caller_uid)
            .await
            .is_some())
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

    #[zbus(property)]
    async fn config(&self) -> Config {
        Config::load_from(CONFIG_PATH).unwrap_or_default()
    }

    #[zbus(property)]
    async fn set_config(
        &self,
        #[zbus(header)] header: Option<Header<'_>>,
        new_config: Config,
    ) -> fdo::Result<()> {
        let header =
            header.ok_or_else(|| fdo::Error::Failed("No message header provided".to_string()))?;
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_CONFIG).await?;

        new_config
            .security
            .validate()
            .map_err(|e| fdo::Error::InvalidArgs(e.to_string()))?;

        self.cancel_active_tasks();

        let new_liveness_detector = if new_config.liveness.enabled {
            let path = crate::models::ensure_liveness_model(gaze_core::config::MODELS_DIR)
                .map_err(|e| fdo::Error::Failed(format!("Failed to ensure liveness model: {e}")))?;
            Some(
                LivenessDetector::new(path.to_str().unwrap()).map_err(|e| {
                    fdo::Error::Failed(format!("Failed to load liveness model: {e}"))
                })?,
            )
        } else {
            None
        };

        let mut threshold = self.threshold.lock().await;
        *threshold = new_config.security.threshold();

        let sources = resolve_configured_sources(&new_config.cameras);
        *self.rgb_device.lock().await = sources.rgb;
        *self.ir_device.lock().await = sources.ir;
        *self.ir_node.lock().await = sources.ir_node;
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

        {
            let mut db = self.db.lock().await;
            db.set_max_templates(new_config.enrollment.max_templates as usize);
        }

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

        {
            let mut detector = self.detector.lock().unwrap_or_else(|e| e.into_inner());
            match gaze_core::detect::FaceDetector::new(det_path.to_str().unwrap()) {
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
            match crate::recognize::FaceRecognizer::new(rec_path.to_str().unwrap()) {
                Ok(rec_rgb) => {
                    let rec_ir =
                        match crate::recognize::FaceRecognizer::new(rec_path.to_str().unwrap()) {
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

    async fn get_gdm_face_auth(&self) -> fdo::Result<bool> {
        Ok(std::path::Path::new(GDM_DCONF_OVERRIDE_PATH).exists())
    }

    async fn set_gdm_face_auth(
        &self,
        #[zbus(header)] header: Header<'_>,
        enabled: bool,
    ) -> fdo::Result<bool> {
        Self::ensure_authorized(&header, POLKIT_ACTION_MANAGE_GDM_PROFILE).await?;

        let path = std::path::Path::new(GDM_DCONF_OVERRIDE_PATH);
        if enabled {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    fdo::Error::Failed(format!("Failed to create {}: {e}", parent.display()))
                })?;
            }
            std::fs::write(path, GDM_DCONF_OVERRIDE_CONTENT).map_err(|e| {
                fdo::Error::Failed(format!("Failed to write {GDM_DCONF_OVERRIDE_PATH}: {e}"))
            })?;
        } else if path.exists() {
            std::fs::remove_file(path).map_err(|e| {
                fdo::Error::Failed(format!("Failed to remove {GDM_DCONF_OVERRIDE_PATH}: {e}"))
            })?;
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
    async fn verify_status(
        ctxt: &SignalEmitter<'_>,
        result: VerifyResult,
        faces: Vec<(String, f64, f64, bool, f64, f64, bool)>,
        rgb_status: CaptureStatus,
        ir_status: CaptureStatus,
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
