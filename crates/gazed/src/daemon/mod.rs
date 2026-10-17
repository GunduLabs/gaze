// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::Context;
use futures::StreamExt;
use ndarray::Array1;
use opencv::core::Mat;
use std::collections::HashMap;
use std::ffi::CString;
use std::future::{Future, pending};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, OwnedMutexGuard, oneshot};
use tracing::{error, info, warn};
use zbus::names::BusName;
use zbus::{fdo, message::Header, object_server::SignalEmitter};

use crate::align::{align_face, mat_to_rgb};
use crate::duress::{DuressLockout, DuressTracker, EyeStateClassifier};
use crate::liveness::LivenessDetector;
use crate::preview::PreviewStream;
use crate::recognize::FaceRecognizer;
use crate::users::{UserDatabase, UserDbError};
use gaze_core::config::{CONFIG_PATH, Config};
use gaze_core::dbus::{CaptureStatus, DbusConfig, EnrollPrompt, VerifyResult};
use gaze_core::desktop::{
    GDM_DCONF_FACE_AUTH_KEY, GDM_FACE_OVERRIDE_PATH, gdm_face_auth_from_dconf,
    gdm_greeter_dconf_source,
};
use gaze_core::ir::led::IrLed;
use gaze_vision::camera::{Camera, CameraKind, resolve_configured_sources};
use gaze_vision::detect::FaceDetector;
use gaze_vision::face::{
    EnrollmentPoseStability, FaceChecker, IrDarkFrameGate, IrFrameKind, RgbFrameKind,
    RgbWarmupGate, Spectrum, enrollment_pose_matches,
};

mod access;
mod benchmark;
mod config;
mod interface;
mod verify;
mod watch;

use benchmark::*;
pub use config::ConfigState;
use config::*;
pub use verify::*;
pub use watch::*;

const POLKIT_ACTION_MANAGE_FACES: &str = "com.gundulabs.gaze.manage-faces";
const POLKIT_ACTION_MANAGE_CONFIG: &str = "com.gundulabs.gaze.manage-config";
const POLKIT_ACTION_MANAGE_GDM_PROFILE: &str = "com.gundulabs.gaze.manage-gdm-profile";
const POLKIT_ACTION_CLEAR_DURESS: &str = "com.gundulabs.gaze.clear-duress";
const GDM_DCONF_OVERRIDE_CONTENT: &str =
    "[org/gnome/shell/extensions/gaze]\nenable-face-authentication=true\n";
const CLAIM_TIMEOUT_SECS: u64 = 300;
const VERIFY_TOO_DARK_TIMEOUT: Duration = Duration::from_secs(1);
const LIVENESS_GATE_DIAGNOSTIC: &str = "Face matched, but the liveness check did not pass. Move slightly and try again, or lower liveness.threshold.";
const VERIFY_NO_FACE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bounds a face that stays badly framed: it refreshes the no-face deadline without ever
/// yielding an embedding. Kept under `CAMERA_AUTH_TIMEOUT_SECS` in `pam-gaze`.
const VERIFY_NO_USABLE_TIMEOUT: Duration = Duration::from_secs(8);
/// Hybrid verify runs one camera at a time for single-function UVC devices (e.g. Logitech
/// Brio). Caps the RGB phase so it yields to IR even without a match. See `verify_start`.
const VERIFY_SERIAL_RGB_BUDGET: Duration = Duration::from_secs(4);
const VERIFY_WATCHDOG_POLL: Duration = Duration::from_millis(250);
const LID_SLEEP_WAIT: Duration = Duration::from_secs(3);
const SSH_PROC_CHAIN_MAX_DEPTH: usize = 16;

/// Aborts the claim's timeout task once the last copy of the claim is gone.
pub struct ClaimTimeout(tokio::task::AbortHandle);

impl Drop for ClaimTimeout {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[derive(Clone)]
pub struct ClaimState {
    pub username: String,
    pub sender: String,
    pub epoch: u64,
    pub _timeout: Option<Arc<ClaimTimeout>>,
}

/// The single claim the daemon will honour, if any.
pub type ClaimStateHandle = Arc<Mutex<Option<ClaimState>>>;

/// Cancellation channel for whatever task the current claim owns.
pub type ActiveCancelHandle = Arc<Mutex<Option<oneshot::Sender<()>>>>;

/// Internal PAM services per owner uid, kept per registering D-Bus sender so a session that
/// ends without cleaning up loses its entries when its connection goes away.
pub type PamInternalSets =
    Arc<Mutex<HashMap<u32, HashMap<String, std::collections::HashSet<String>>>>>;

/// Held through capture-thread teardown.
pub type CameraLease = Arc<Mutex<()>>;

#[derive(Debug, PartialEq, Eq)]
enum CameraLeaseError {
    Cancelled,
    SleepChanged,
    AccessDenied,
}

/// Detect coalesced suspend/resume cycles even when the final value is false.
fn take_sleep_change(
    sleeping: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<bool, CameraLeaseError> {
    sleeping
        .has_changed()
        .map_err(|_| CameraLeaseError::Cancelled)?;
    let state = sleeping.borrow_and_update();
    Ok(state.has_changed() || *state)
}

/// Recheck authorization after acquiring the lease; both waits are cancellable.
async fn acquire_camera_lease(
    lease: &CameraLease,
    cancel: &mut oneshot::Receiver<()>,
    mut sleeping: Option<&mut tokio::sync::watch::Receiver<bool>>,
    authorized: impl Future<Output = bool>,
) -> Result<OwnedMutexGuard<()>, CameraLeaseError> {
    if let Some(receiver) = sleeping.as_deref_mut()
        && take_sleep_change(receiver)?
    {
        return Err(CameraLeaseError::SleepChanged);
    }

    let result = tokio::select! {
        biased;
        _ = &mut *cancel => Err(CameraLeaseError::Cancelled),
        changed = async {
            match sleeping.as_deref_mut() {
                Some(receiver) => receiver.changed().await,
                None => pending().await,
            }
        } => {
            match changed {
                Ok(()) => Err(CameraLeaseError::SleepChanged),
                Err(_) => Err(CameraLeaseError::Cancelled),
            }
        }
        result = async {
            let guard = lease.clone().lock_owned().await;
            if authorized.await {
                Ok(guard)
            } else {
                Err(CameraLeaseError::AccessDenied)
            }
        } => result,
    };

    // Catch notifications arriving during authorization's final poll.
    if cancel.try_recv() != Err(oneshot::error::TryRecvError::Empty) {
        return Err(CameraLeaseError::Cancelled);
    }
    if let Some(receiver) = sleeping
        && take_sleep_change(receiver)?
    {
        return Err(CameraLeaseError::SleepChanged);
    }
    result
}

fn forget_pam_internal_sender(
    sets: &mut HashMap<u32, HashMap<String, std::collections::HashSet<String>>>,
    sender: &str,
) -> bool {
    let mut removed = false;
    sets.retain(|_, senders| {
        removed |= senders.remove(sender).is_some();
        !senders.is_empty()
    });
    removed
}

static CLAIM_EPOCH: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interaction {
    Allow,
    Deny,
}
static SYSTEM_BUS: tokio::sync::OnceCell<zbus::Connection> = tokio::sync::OnceCell::const_new();
static DBUS_PROXY: tokio::sync::OnceCell<fdo::DBusProxy<'static>> =
    tokio::sync::OnceCell::const_new();

pub async fn system_bus() -> fdo::Result<zbus::Connection> {
    SYSTEM_BUS
        .get_or_try_init(zbus::Connection::system)
        .await
        .cloned()
        .map_err(|e| fdo::Error::Failed(format!("Failed to connect to system bus: {e}")))
}

async fn active_session() -> Option<gaze_core::dbus::ActiveSession> {
    let conn = system_bus().await.ok()?;
    gaze_core::dbus::active_session_on(&conn).await.ok()
}

async fn active_session_uid_and_class() -> Option<(u32, String)> {
    let conn = system_bus().await.ok()?;
    gaze_core::dbus::active_session_uid_and_class_on(&conn)
        .await
        .ok()
}

async fn dbus_proxy() -> fdo::Result<&'static fdo::DBusProxy<'static>> {
    DBUS_PROXY
        .get_or_try_init(|| async {
            let conn = system_bus().await?;
            fdo::DBusProxy::new(&conn)
                .await
                .map_err(|e| fdo::Error::Failed(format!("Failed to create DBus proxy: {e}")))
        })
        .await
}

fn claim_has_epoch(state: &Option<ClaimState>, epoch: u64) -> bool {
    matches!(state, Some(claim) if claim.epoch == epoch)
}

// Hold the claim lock while replacing its cancellation channel. Authorization and
// camera/config lookups may await long enough for this claim to be revoked.
async fn replace_claim_task(
    claim_state: &ClaimStateHandle,
    active_cancel: &ActiveCancelHandle,
    epoch: u64,
) -> fdo::Result<oneshot::Receiver<()>> {
    let state = claim_state.lock().await;
    if !claim_has_epoch(&state, epoch) {
        return Err(fdo::Error::AccessDenied("Daemon claim was revoked".into()));
    }
    let mut cancel = active_cancel.lock().await;
    if let Some(previous) = cancel.take() {
        let _ = previous.send(());
    }
    let (tx, rx) = oneshot::channel();
    *cancel = Some(tx);
    Ok(rx)
}

async fn cancel_claim_task(
    claim_state: &ClaimStateHandle,
    active_cancel: &ActiveCancelHandle,
    epoch: u64,
) -> fdo::Result<()> {
    let state = claim_state.lock().await;
    if !claim_has_epoch(&state, epoch) {
        return Err(fdo::Error::AccessDenied("Daemon claim was revoked".into()));
    }
    if let Some(tx) = active_cancel.lock().await.take() {
        let _ = tx.send(());
    }
    Ok(())
}

/// Whether a NameOwnerChanged signal says `watched` lost its owner. A `new_owner`
/// means the name was acquired or handed on, not that the client went away.
fn is_vanish_of(name: &str, new_owner: Option<&str>, watched: &str) -> bool {
    name == watched && new_owner.is_none()
}

/// Drop the claim identified by `epoch`, cancel its task, and report whether this call
/// is what dropped it. Epochs are unique per claim; unique names are not.
async fn release_claim_epoch(
    claim_state: &ClaimStateHandle,
    active_cancel: &ActiveCancelHandle,
    epoch: u64,
) -> bool {
    let mut state = claim_state.lock().await;
    if !claim_has_epoch(&state, epoch) {
        return false;
    }
    // Dropped only after the last await: the timeout task calls this, and dropping the claim
    // aborts that task at its next await.
    let released = state.take();
    let mut cancel = active_cancel.lock().await;
    if let Some(tx) = cancel.take() {
        let _ = tx.send(());
    }
    drop(released);
    true
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
    activation_message: Option<String>,
    refresh_pending: bool,
}

static EMITTER_USERS: std::sync::OnceLock<std::sync::Mutex<HashMap<String, usize>>> =
    std::sync::OnceLock::new();

fn emitter_users() -> std::sync::MutexGuard<'static, HashMap<String, usize>> {
    EMITTER_USERS
        .get_or_init(|| std::sync::Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

impl EmitterGuard {
    fn engage(kind: &CameraKind, enabled: bool) -> Self {
        let mut activation_message = None;
        let mut refresh_pending = false;
        let led = match kind {
            CameraKind::Ir { node, .. } if enabled => match IrLed::for_path(node) {
                Some(led) => {
                    *emitter_users().entry(led.node().to_string()).or_insert(0) += 1;
                    refresh_pending = led.needs_stream_refresh();
                    match led.set(true) {
                        Err(e) if refresh_pending => {
                            info!(
                                "IR emitter not yet reachable, retrying once the stream starts: {e}"
                            );
                        }
                        Err(e) => warn!("IR emitter activate failed: {e}"),
                        Ok(()) => {
                            let message = format!(
                                "IR emitter enabled via {} on {}",
                                led.device_name(),
                                led.node()
                            );
                            info!("{message}");
                            activation_message = Some(message);
                        }
                    }
                    Some(led)
                }
                None => {
                    warn!("No IR emitter profile for {node}; continuing without illumination");
                    None
                }
            },
            _ => None,
        };
        Self {
            led,
            activation_message,
            refresh_pending,
        }
    }

    fn activation_message(&self) -> Option<&str> {
        self.activation_message.as_deref()
    }

    fn stream_started(&mut self) {
        if !std::mem::take(&mut self.refresh_pending) {
            return;
        }
        if let Some(led) = &self.led {
            match led.set(true) {
                Ok(()) => info!(
                    "IR emitter re-applied via {} after the stream started",
                    led.device_name()
                ),
                Err(e) => warn!("IR emitter activate failed after the stream started: {e}"),
            }
        }
    }
}

impl Drop for EmitterGuard {
    fn drop(&mut self) {
        let Some(led) = &self.led else {
            return;
        };
        let mut users = emitter_users();
        let remaining = match users.get_mut(led.node()) {
            Some(count) if *count > 1 => {
                *count -= 1;
                *count
            }
            _ => {
                users.remove(led.node());
                0
            }
        };
        drop(users);
        if remaining == 0
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

#[derive(Clone)]
pub struct CaptureModels {
    pub detector: Arc<std::sync::Mutex<FaceDetector>>,
    pub recognizer: Arc<Mutex<FaceRecognizer>>,
}

impl CaptureModels {
    fn load(
        detector: &gaze_vision::inference::ModelFile,
        recognizer: &gaze_vision::inference::ModelFile,
        inference: &gaze_core::config::InferenceConfig,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            detector: Arc::new(std::sync::Mutex::new(
                FaceDetector::new_with_inference(detector, inference)
                    .context("Failed to load detector")?,
            )),
            recognizer: Arc::new(Mutex::new(
                FaceRecognizer::new_with_inference(recognizer, inference)
                    .context("Failed to load recognizer")?,
            )),
        })
    }
}

#[derive(Clone)]
pub struct ModelSessions {
    pub rgb: CaptureModels,
    pub ir: Option<CaptureModels>,
}

fn ir_models_needed(config: &Config) -> bool {
    !config.cameras.ir.trim().is_empty()
}

impl ModelSessions {
    pub fn load(
        detector: &gaze_vision::inference::ModelFile,
        recognizer: &gaze_vision::inference::ModelFile,
        config: &Config,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            rgb: CaptureModels::load(detector, recognizer, &config.inference)
                .context("RGB model loading failed")?,
            ir: if ir_models_needed(config) {
                Some(
                    CaptureModels::load(detector, recognizer, &config.inference)
                        .context("IR model loading failed")?,
                )
            } else {
                None
            },
        })
    }
}

pub struct AuthDaemon {
    pub models: Arc<std::sync::Mutex<ModelSessions>>,
    pub liveness: Arc<Mutex<Option<LivenessDetector>>>,
    pub eye_state: Arc<Mutex<Option<EyeStateClassifier>>>,
    pub duress_lockout: Arc<DuressLockout>,
    pub db: Arc<Mutex<UserDatabase>>,
    pub settings: ConfigState,
    pub claim_state: ClaimStateHandle,
    pub active_cancel: ActiveCancelHandle,
    pub pam_internal: PamInternalSets,
    pub camera_lease: CameraLease,
    pub resume_pending: Arc<AtomicBool>,
    pub resume_seen: Arc<AtomicBool>,
    pub sleeping: tokio::sync::watch::Receiver<bool>,
    pub lock_epochs: LockEpochs,
    pub benchmark_running: Arc<AtomicBool>,
    pub rt_handle: tokio::runtime::Handle,
}

impl AuthDaemon {
    /// Capture callers hold an applied-config read guard through teardown.
    fn capture_models(&self, config: &Config) -> fdo::Result<ModelSessions> {
        let models = self
            .models
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if ir_models_needed(config) && models.ir.is_none() {
            return Err(fdo::Error::Failed(
                "IR camera is configured but its models are unavailable".into(),
            ));
        }
        Ok(models)
    }
}

fn validate_keyring_verification(
    required: bool,
    liveness: &gaze_core::config::LivenessConfig,
    templates_encrypted: bool,
) -> fdo::Result<()> {
    gaze_core::config::StorageConfig {
        encrypt_templates: templates_encrypted,
        unlock_gnome_keyring: required,
        unlock_kwallet: false,
    }
    .validate_keyring(liveness)
    .map_err(|e| fdo::Error::Failed(format!("Keyring verification unavailable: {e}")))
}

/// When each logind session last became locked, keyed by session object path.
pub type LockEpochs = Arc<Mutex<HashMap<String, std::time::Instant>>>;

struct BenchmarkSlot(Arc<AtomicBool>);

impl BenchmarkSlot {
    fn acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self(flag.clone()))
    }
}

impl Drop for BenchmarkSlot {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::oneshot::error::TryRecvError;
    use tokio::sync::{Mutex, oneshot, watch};

    #[test]
    fn ir_models_are_needed_only_for_a_configured_ir_source() {
        let mut config = Config::default();
        for source in ["", " ", "\t"] {
            config.cameras.ir = source.into();
            assert!(!ir_models_needed(&config));
        }
        for source in [
            "/dev/video2",
            "usb:1234:5678",
            "pipewiresrc target-object=ir",
        ] {
            config.cameras.ir = source.into();
            assert!(ir_models_needed(&config));
        }
    }

    #[test]
    fn invalid_models_return_an_error_instead_of_panicking_at_startup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("invalid.onnx");
        std::fs::write(&path, b"not an ONNX model").unwrap();
        let model = gaze_vision::inference::ModelFile::verify(path, None).unwrap();
        assert!(ModelSessions::load(&model, &model, &Config::default()).is_err());
    }

    #[test]
    fn rgb_only_skips_ir_and_dual_spectrum_uses_independent_sessions() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("identity.onnx");
        let identity = [
            8, 8, 18, 4, 103, 97, 122, 101, 58, 87, 10, 16, 10, 1, 120, 18, 1, 121, 34, 8, 73, 100,
            101, 110, 116, 105, 116, 121, 18, 9, 103, 97, 122, 101, 45, 116, 101, 115, 116, 90, 27,
            10, 1, 120, 18, 22, 10, 20, 8, 1, 18, 16, 10, 2, 8, 1, 10, 2, 8, 3, 10, 2, 8, 2, 10, 2,
            8, 2, 98, 27, 10, 1, 121, 18, 22, 10, 20, 8, 1, 18, 16, 10, 2, 8, 1, 10, 2, 8, 3, 10,
            2, 8, 2, 10, 2, 8, 2, 66, 2, 16, 13,
        ];
        std::fs::write(&path, identity).unwrap();
        let model = gaze_vision::inference::ModelFile::verify(path, None).unwrap();
        let mut config = Config::default();
        let rgb_only = ModelSessions::load(&model, &model, &config).unwrap();
        assert!(rgb_only.ir.is_none());
        config.cameras.ir = "/dev/video2".into();
        let dual = ModelSessions::load(&model, &model, &config).unwrap();
        let ir = dual.ir.as_ref().unwrap();
        assert!(!Arc::ptr_eq(&dual.rgb.detector, &ir.detector));
        assert!(!Arc::ptr_eq(&dual.rgb.recognizer, &ir.recognizer));
    }

    #[tokio::test]
    async fn seat_authorization_is_rechecked_after_the_previous_capture_stops() {
        let lease = Arc::new(Mutex::new(()));
        let previous = lease.clone().lock_owned().await;
        let allowed = AtomicBool::new(true);
        let checked = AtomicBool::new(false);
        let (_cancel_tx, mut cancel) = oneshot::channel();
        let attempt = acquire_camera_lease(&lease, &mut cancel, None, async {
            checked.store(true, Ordering::SeqCst);
            allowed.load(Ordering::SeqCst)
        });
        tokio::pin!(attempt);
        assert!(futures::poll!(&mut attempt).is_pending());
        assert!(!checked.load(Ordering::SeqCst));

        allowed.store(false, Ordering::SeqCst);
        drop(previous);
        assert_eq!(attempt.await.unwrap_err(), CameraLeaseError::AccessDenied);
        assert!(checked.load(Ordering::SeqCst));
        assert!(lease.try_lock().is_ok());
    }

    #[tokio::test]
    async fn cancellation_while_waiting_never_runs_authorization() {
        let lease = Arc::new(Mutex::new(()));
        let previous = lease.clone().lock_owned().await;
        let checked = AtomicBool::new(false);
        let (cancel_tx, mut cancel) = oneshot::channel();
        let attempt = acquire_camera_lease(&lease, &mut cancel, None, async {
            checked.store(true, Ordering::SeqCst);
            true
        });
        tokio::pin!(attempt);
        assert!(futures::poll!(&mut attempt).is_pending());
        cancel_tx.send(()).unwrap();
        assert_eq!(attempt.await.unwrap_err(), CameraLeaseError::Cancelled);
        assert!(!checked.load(Ordering::SeqCst));
        drop(previous);
    }

    #[tokio::test]
    async fn cancellation_during_authorization_releases_the_lease() {
        let lease = Arc::new(Mutex::new(()));
        let (cancel_tx, mut cancel) = oneshot::channel();
        let attempt = acquire_camera_lease(&lease, &mut cancel, None, pending());
        tokio::pin!(attempt);
        assert!(futures::poll!(&mut attempt).is_pending());
        assert!(lease.try_lock().is_err());
        cancel_tx.send(()).unwrap();
        assert_eq!(attempt.await.unwrap_err(), CameraLeaseError::Cancelled);
        assert!(lease.try_lock().is_ok());
    }

    #[tokio::test]
    async fn suspend_while_waiting_does_not_acquire_the_camera() {
        let lease = Arc::new(Mutex::new(()));
        let previous = lease.clone().lock_owned().await;
        let (_cancel_tx, mut cancel) = oneshot::channel();
        let (sleep_tx, mut sleeping) = watch::channel(false);
        let attempt =
            acquire_camera_lease(&lease, &mut cancel, Some(&mut sleeping), async { true });
        tokio::pin!(attempt);
        assert!(futures::poll!(&mut attempt).is_pending());
        sleep_tx.send_replace(true);
        assert_eq!(attempt.await.unwrap_err(), CameraLeaseError::SleepChanged);
        drop(previous);
    }

    #[tokio::test]
    async fn a_coalesced_suspend_resume_cycle_interrupts_the_lease_wait() {
        let lease = Arc::new(Mutex::new(()));
        let previous = lease.clone().lock_owned().await;
        let (_cancel_tx, mut cancel) = oneshot::channel();
        let (sleep_tx, mut sleeping) = watch::channel(false);
        let attempt =
            acquire_camera_lease(&lease, &mut cancel, Some(&mut sleeping), async { true });
        tokio::pin!(attempt);
        assert!(futures::poll!(&mut attempt).is_pending());
        sleep_tx.send_replace(true);
        sleep_tx.send_replace(false);
        drop(previous);
        assert_eq!(attempt.await.unwrap_err(), CameraLeaseError::SleepChanged);
        assert!(lease.try_lock().is_ok());
    }

    #[tokio::test]
    async fn suspend_during_authorization_releases_the_lease() {
        let lease = Arc::new(Mutex::new(()));
        let (_cancel_tx, mut cancel) = oneshot::channel();
        let (sleep_tx, mut sleeping) = watch::channel(false);
        let attempt = acquire_camera_lease(&lease, &mut cancel, Some(&mut sleeping), pending());
        tokio::pin!(attempt);
        assert!(futures::poll!(&mut attempt).is_pending());
        assert!(lease.try_lock().is_err());
        sleep_tx.send_replace(true);
        assert_eq!(attempt.await.unwrap_err(), CameraLeaseError::SleepChanged);
        assert!(lease.try_lock().is_ok());
    }

    #[tokio::test]
    async fn successful_authorization_holds_the_lease_until_teardown_finishes() {
        let lease = Arc::new(Mutex::new(()));
        let (_cancel_tx, mut cancel) = oneshot::channel();
        let guard = acquire_camera_lease(&lease, &mut cancel, None, async { true })
            .await
            .unwrap();
        assert!(lease.try_lock().is_err());
        drop(guard);
        assert!(lease.try_lock().is_ok());
    }

    #[tokio::test]
    async fn a_resume_in_the_final_authorization_poll_is_not_missed() {
        let lease = Arc::new(Mutex::new(()));
        let (_cancel_tx, mut cancel) = oneshot::channel();
        let (sleep_tx, mut sleeping) = watch::channel(false);
        let result = acquire_camera_lease(&lease, &mut cancel, Some(&mut sleeping), async {
            sleep_tx.send_replace(true);
            sleep_tx.send_replace(false);
            true
        })
        .await;
        assert_eq!(result.unwrap_err(), CameraLeaseError::SleepChanged);
        assert!(lease.try_lock().is_ok());
    }

    #[tokio::test]
    async fn cancellation_in_the_final_authorization_poll_is_not_missed() {
        let lease = Arc::new(Mutex::new(()));
        let (cancel_tx, mut cancel) = oneshot::channel();
        let result = acquire_camera_lease(&lease, &mut cancel, None, async {
            cancel_tx.send(()).unwrap();
            true
        })
        .await;
        assert_eq!(result.unwrap_err(), CameraLeaseError::Cancelled);
        assert!(lease.try_lock().is_ok());
    }

    #[test]
    fn lease_future_can_run_on_the_multithreaded_daemon_runtime() {
        fn assert_send(_: impl Send) {}
        let lease = Arc::new(Mutex::new(()));
        let (_cancel_tx, mut cancel) = oneshot::channel();
        let (_sleep_tx, mut sleeping) = watch::channel(false);
        assert_send(acquire_camera_lease(
            &lease,
            &mut cancel,
            Some(&mut sleeping),
            async { true },
        ));
    }

    #[tokio::test]
    async fn a_closed_sleep_watcher_fails_closed() {
        let lease = Arc::new(Mutex::new(()));
        let (_cancel_tx, mut cancel) = oneshot::channel();
        let (sleep_tx, mut sleeping) = watch::channel(false);
        drop(sleep_tx);
        assert_eq!(
            acquire_camera_lease(&lease, &mut cancel, Some(&mut sleeping), async { true })
                .await
                .unwrap_err(),
            CameraLeaseError::Cancelled,
        );
        assert!(lease.try_lock().is_ok());
    }

    #[test]
    fn a_vanished_sender_loses_only_its_internal_services() {
        let mut sets: HashMap<u32, HashMap<String, std::collections::HashSet<String>>> =
            HashMap::new();
        let owner = sets.entry(1000).or_default();
        owner.insert(":1.10".to_string(), ["polkit-1".to_string()].into());
        owner.insert(":1.11".to_string(), ["gdm-face".to_string()].into());

        assert!(forget_pam_internal_sender(&mut sets, ":1.10"));
        assert_eq!(sets[&1000].len(), 1);
        assert!(forget_pam_internal_sender(&mut sets, ":1.11"));
        assert!(sets.is_empty());
        assert!(!forget_pam_internal_sender(&mut sets, ":1.11"));
    }

    #[tokio::test]
    async fn releasing_a_claim_aborts_its_timeout() {
        let timeout = tokio::spawn(std::future::pending::<()>());
        let claim_state = Arc::new(Mutex::new(Some(ClaimState {
            username: "alice".to_string(),
            sender: ":1.42".to_string(),
            epoch: 7,
            _timeout: Some(Arc::new(ClaimTimeout(timeout.abort_handle()))),
        })));
        let active_cancel = Arc::new(Mutex::new(None));

        assert!(release_claim_epoch(&claim_state, &active_cancel, 7).await);
        assert!(timeout.await.unwrap_err().is_cancelled());
    }

    #[test]
    fn stale_claim_epoch_does_not_match_reclaimed_state() {
        let state = Some(ClaimState {
            username: "alice".to_string(),
            sender: ":1.42".to_string(),
            epoch: 2,
            _timeout: None,
        });

        assert!(!claim_has_epoch(&state, 1));
        assert!(claim_has_epoch(&state, 2));
    }

    fn claim_at(epoch: u64) -> ClaimStateHandle {
        Arc::new(Mutex::new(Some(ClaimState {
            username: "alice".to_string(),
            sender: ":1.42".to_string(),
            epoch,
            _timeout: None,
        })))
    }

    fn hardened_config() -> gaze_core::config::Config {
        let mut config = gaze_core::config::Config::default();
        config.security = gaze_core::config::SecurityLevel::maximum();
        config.auth.require_confirmation_lock_screen = true;
        config.auth.require_confirmation_elevation = true;
        config
    }

    #[test]
    fn keyring_verification_uses_active_state_even_when_disk_settings_are_enabled() {
        let mut disk = gaze_core::config::Config::default();
        disk.storage.encrypt_templates = true;
        disk.storage.unlock_gnome_keyring = true;
        disk.liveness.enabled = true;
        assert!(disk.storage.validate_keyring(&disk.liveness).is_ok());

        for liveness in [false, true] {
            for encryption in [false, true] {
                let active_liveness = gaze_core::config::LivenessConfig {
                    enabled: liveness,
                    ..disk.liveness.clone()
                };
                assert_eq!(
                    super::validate_keyring_verification(true, &active_liveness, encryption)
                        .is_ok(),
                    liveness && encryption
                );
                assert!(
                    super::validate_keyring_verification(false, &active_liveness, encryption)
                        .is_ok()
                );
            }
        }
    }

    #[tokio::test]
    async fn a_hand_edited_keyring_opt_in_is_ignored_rather_than_breaking_face_login() {
        let mut on_disk = Config::default();
        on_disk.storage.unlock_gnome_keyring = true;
        on_disk.storage.encrypt_templates = false;
        on_disk.liveness.enabled = true;
        let settings = ConfigState::new(on_disk);
        assert!(!settings.read().await.storage.unlock_gnome_keyring);
        assert!(!settings.read().await.storage.unlock_gnome_keyring);

        let mut valid = Config::default();
        valid.storage.unlock_gnome_keyring = true;
        let settings = ConfigState::new(valid);
        assert!(settings.read().await.storage.unlock_gnome_keyring);
    }

    #[tokio::test]
    async fn a_missing_or_unreadable_file_keeps_the_running_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let path = path.to_str().unwrap();
        hardened_config().save_to(path).unwrap();
        let settings = ConfigState::new(Config::load_from(path).unwrap());
        std::fs::remove_file(path).unwrap();
        for unreadable in [false, true] {
            if unreadable {
                std::fs::write(path, "[broken").unwrap();
            }
            let current = settings.read().await;
            assert_eq!(current.security.level, "maximum");
            assert!(current.auth.require_confirmation_lock_screen);
            assert!(current.auth.require_confirmation_elevation);
        }
    }

    #[test]
    fn defaults_would_have_weakened_the_running_settings() {
        let defaults = gaze_core::config::Config::default();
        let hardened = hardened_config();

        assert_ne!(defaults.security.level, hardened.security.level);
        assert!(!defaults.auth.require_confirmation_lock_screen);
        assert!(!defaults.auth.require_confirmation_elevation);
    }

    #[tokio::test]
    async fn disk_edits_do_not_change_the_advertised_or_running_settings() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let path = path.to_str().unwrap();
        hardened_config().save_to(path).unwrap();
        let settings = ConfigState::new(Config::load_from(path).unwrap());
        Config::default().save_to(path).unwrap();
        assert_eq!(Config::load_from(path).unwrap().security.level, "medium");
        assert_eq!(settings.read().await.security.level, "maximum");
    }

    #[tokio::test]
    async fn system_bus_is_reused_across_calls() {
        let Ok(first) = super::system_bus().await else {
            return;
        };
        let second = super::system_bus().await.expect("cached bus");
        assert_eq!(
            first.unique_name(),
            second.unique_name(),
            "every caller must share one connection"
        );

        let fresh = zbus::Connection::system()
            .await
            .expect("a second connection must still be possible");
        assert_ne!(
            first.unique_name(),
            fresh.unique_name(),
            "a distinct connection is what the cache exists to avoid"
        );
    }

    #[tokio::test]
    async fn revoked_claim_cannot_launch_a_capture() {
        let state = claim_at(7);
        let cancel = Arc::new(Mutex::new(None));
        assert!(release_claim_epoch(&state, &cancel, 7).await);
        assert!(replace_claim_task(&state, &cancel, 7).await.is_err());
        assert!(cancel.lock().await.is_none());
    }

    #[tokio::test]
    async fn stale_start_and_stop_preserve_the_new_owners_capture() {
        let state = claim_at(8);
        let cancel = Arc::new(Mutex::new(None));
        let mut current = replace_claim_task(&state, &cancel, 8).await.unwrap();
        assert!(replace_claim_task(&state, &cancel, 7).await.is_err());
        assert!(cancel_claim_task(&state, &cancel, 7).await.is_err());
        assert_eq!(current.try_recv(), Err(oneshot::error::TryRecvError::Empty));
        assert!(cancel.lock().await.is_some());
        cancel_claim_task(&state, &cancel, 8).await.unwrap();
        assert_eq!(current.await, Ok(()));
    }

    #[tokio::test]
    async fn replacing_a_capture_cancels_the_previous_one_and_remains_revocable() {
        let state = claim_at(9);
        let cancel = Arc::new(Mutex::new(None));
        let previous = replace_claim_task(&state, &cancel, 9).await.unwrap();
        let current = replace_claim_task(&state, &cancel, 9).await.unwrap();
        assert_eq!(previous.await, Ok(()));
        assert!(release_claim_epoch(&state, &cancel, 9).await);
        assert_eq!(current.await, Ok(()));
    }

    // The vanish watcher, the owner re-check, and the claim timeout all release here.
    #[tokio::test]
    async fn release_clears_and_cancels() {
        let claim_state = claim_at(7);
        let (tx, mut rx) = oneshot::channel();
        let active_cancel = Arc::new(Mutex::new(Some(tx)));

        assert!(release_claim_epoch(&claim_state, &active_cancel, 7).await);
        assert!(claim_state.lock().await.is_none());
        assert!(rx.try_recv().is_ok(), "the active task must be cancelled");
    }

    // A watcher spawned for an earlier claim must not revoke the one that replaced it.
    #[tokio::test]
    async fn stale_epoch_spares_newer_claim() {
        let claim_state = claim_at(8);
        let (tx, mut rx) = oneshot::channel();
        let active_cancel = Arc::new(Mutex::new(Some(tx)));

        assert!(!release_claim_epoch(&claim_state, &active_cancel, 7).await);
        assert!(claim_has_epoch(&*claim_state.lock().await, 8));
        // Empty, not just Err, because a dropped sender also reports Err but leaves the
        // newer claim's task uncancellable.
        assert!(
            matches!(rx.try_recv(), Err(TryRecvError::Empty)),
            "the newer claim's task must not be cancelled"
        );
    }

    // The owner re-check and the signal handler can both fire for one claim.
    #[tokio::test]
    async fn double_release_is_idempotent() {
        let claim_state = claim_at(9);
        let (tx, _rx) = oneshot::channel();
        let active_cancel = Arc::new(Mutex::new(Some(tx)));

        assert!(release_claim_epoch(&claim_state, &active_cancel, 9).await);
        assert!(!release_claim_epoch(&claim_state, &active_cancel, 9).await);
        assert!(claim_state.lock().await.is_none());
    }

    // A claim held with no verification running still has to clear.
    #[tokio::test]
    async fn release_without_an_active_task_still_clears() {
        let claim_state = claim_at(3);
        let active_cancel = Arc::new(Mutex::new(None));

        assert!(release_claim_epoch(&claim_state, &active_cancel, 3).await);
        assert!(claim_state.lock().await.is_none());
    }

    #[tokio::test]
    async fn release_on_an_unclaimed_daemon_is_a_noop() {
        let claim_state = Arc::new(Mutex::new(None));
        let (tx, mut rx) = oneshot::channel();
        let active_cancel = Arc::new(Mutex::new(Some(tx)));

        assert!(!release_claim_epoch(&claim_state, &active_cancel, 1).await);
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    // The case the epoch guard exists for, where one connection claims, releases, and
    // claims again, so the same unique name backs two different claims.
    #[tokio::test]
    async fn same_sender_reclaiming_is_not_released_by_the_old_epoch() {
        let claim_state = claim_at(11);
        let active_cancel = Arc::new(Mutex::new(None));

        assert!(release_claim_epoch(&claim_state, &active_cancel, 11).await);
        *claim_state.lock().await = Some(ClaimState {
            username: "alice".to_string(),
            sender: ":1.42".to_string(),
            epoch: 12,
            _timeout: None,
        });

        assert!(!release_claim_epoch(&claim_state, &active_cancel, 11).await);
        assert!(claim_has_epoch(&*claim_state.lock().await, 12));
    }

    // The re-check and the vanish signal race by design; exactly one may win.
    #[tokio::test]
    async fn concurrent_releases_elect_a_single_winner() {
        let claim_state = claim_at(4);
        let (tx, mut rx) = oneshot::channel();
        let active_cancel = Arc::new(Mutex::new(Some(tx)));

        let (a, b) = tokio::join!(
            release_claim_epoch(&claim_state, &active_cancel, 4),
            release_claim_epoch(&claim_state, &active_cancel, 4)
        );

        assert!(a ^ b, "exactly one caller must report the release");
        assert!(claim_state.lock().await.is_none());
        assert!(rx.try_recv().is_ok(), "the active task must be cancelled");
    }

    #[test]
    fn vanish_needs_the_watched_name_and_no_new_owner() {
        assert!(is_vanish_of(":1.42", None, ":1.42"));
        // An acquisition or hand-off is not a disappearance.
        assert!(!is_vanish_of(":1.42", Some(":1.42"), ":1.42"));
        assert!(!is_vanish_of(":1.99", None, ":1.42"));
        // Prefix collision, where ":1.4" vanishing must not release ":1.42".
        assert!(!is_vanish_of(":1.4", None, ":1.42"));
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
    fn emitter_guard_is_inert_for_rgb_and_when_disabled() {
        use super::EmitterGuard;
        use gaze_vision::camera::CameraKind;

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
    fn only_one_benchmark_slot_is_available_at_a_time() {
        use super::BenchmarkSlot;
        use std::sync::atomic::AtomicBool;

        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let first = BenchmarkSlot::acquire(&flag).expect("first caller acquires");
        assert!(BenchmarkSlot::acquire(&flag).is_none());

        drop(first);
        let second = BenchmarkSlot::acquire(&flag).expect("slot is released");
        drop(second);
        assert!(!flag.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn benchmark_slot_is_released_when_the_holder_panics() {
        use super::BenchmarkSlot;
        use std::sync::atomic::AtomicBool;

        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let inner = flag.clone();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let _ = std::panic::catch_unwind(move || {
            let _slot = BenchmarkSlot::acquire(&inner).expect("acquired");
            panic!("benchmark blew up");
        });
        std::panic::set_hook(previous);

        assert!(BenchmarkSlot::acquire(&flag).is_some());
    }
}
