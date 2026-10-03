// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

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
    GDM_DCONF_FACE_AUTH_KEY, GDM_DCONF_PROFILE, GDM_DCONF_PROFILE_PATH, GDM_FACE_OVERRIDE_PATH,
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
use config::*;
pub use verify::*;
pub use watch::*;

const POLKIT_ACTION_MANAGE_FACES: &str = "com.gundulabs.gaze.manage-faces";
const POLKIT_ACTION_MANAGE_CONFIG: &str = "com.gundulabs.gaze.manage-config";
const POLKIT_ACTION_MANAGE_GDM_PROFILE: &str = "com.gundulabs.gaze.manage-gdm-profile";
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
const SSH_PROC_CHAIN_MAX_DEPTH: usize = 16;

#[derive(Clone)]
pub struct ClaimState {
    pub username: String,
    pub sender: String,
    pub epoch: u64,
}

/// The single claim the daemon will honour, if any.
pub type ClaimStateHandle = Arc<Mutex<Option<ClaimState>>>;

/// Cancellation channel for whatever task the current claim owns.
pub type ActiveCancelHandle = Arc<Mutex<Option<oneshot::Sender<()>>>>;

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
    *state = None;
    let mut cancel = active_cancel.lock().await;
    if let Some(tx) = cancel.take() {
        let _ = tx.send(());
    }
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

pub struct AuthDaemon {
    pub detector: Arc<std::sync::Mutex<FaceDetector>>,
    pub recognizer_rgb: Arc<Mutex<FaceRecognizer>>,
    pub recognizer_ir: Arc<Mutex<FaceRecognizer>>,
    pub liveness: Arc<Mutex<Option<LivenessDetector>>>,
    pub eye_state: Arc<Mutex<Option<EyeStateClassifier>>>,
    pub duress_lockout: Arc<DuressLockout>,
    pub db: Arc<Mutex<UserDatabase>>,
    pub rgb_threshold: Arc<Mutex<f32>>,
    pub ir_threshold: Arc<Mutex<f32>>,
    pub rgb_device: Arc<Mutex<String>>,
    pub ir_device: Arc<Mutex<String>>,
    pub ir_node: Arc<Mutex<String>>,
    pub serial_capture: Arc<Mutex<bool>>,
    pub emitter_enabled: Arc<Mutex<bool>>,
    pub liveness_config: Arc<Mutex<gaze_core::config::LivenessConfig>>,
    pub hybrid_policy: Arc<Mutex<String>>,
    pub abort_if_ssh: Arc<Mutex<bool>>,
    pub abort_if_lid_closed: Arc<Mutex<bool>>,
    pub abort_before_first_resume: Arc<Mutex<bool>>,
    pub claim_state: ClaimStateHandle,
    pub active_cancel: ActiveCancelHandle,
    pub active_extensions: Arc<Mutex<std::collections::HashMap<u32, bool>>>,
    pub pam_internal: Arc<Mutex<std::collections::HashMap<u32, std::collections::HashSet<String>>>>,
    pub resume_pending: Arc<AtomicBool>,
    pub resume_seen: Arc<AtomicBool>,
    pub lock_epochs: LockEpochs,
    pub benchmark_running: Arc<AtomicBool>,
    pub last_good_config: Arc<Mutex<Config>>,
    pub rt_handle: tokio::runtime::Handle,
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

fn resolve_config(loaded: anyhow::Result<Config>, last_good: &mut Config) -> Config {
    match loaded {
        Ok(mut config) => {
            if config.clamp_keyring() {
                warn!(
                    path = CONFIG_PATH,
                    "keyring unlock needs storage.encrypt_templates and liveness.enabled; \
                     ignoring the keyring options"
                );
            }
            *last_good = config.clone();
            config
        }
        Err(e) => {
            error!(
                error = %e,
                path = CONFIG_PATH,
                "config is unreadable; keeping the last valid configuration"
            );
            last_good.clone()
        }
    }
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
    use tokio::sync::{Mutex, oneshot};

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

    fn claim_at(epoch: u64) -> ClaimStateHandle {
        Arc::new(Mutex::new(Some(ClaimState {
            username: "alice".to_string(),
            sender: ":1.42".to_string(),
            epoch,
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

    #[test]
    fn a_hand_edited_keyring_opt_in_is_ignored_rather_than_breaking_face_login() {
        let mut on_disk = gaze_core::config::Config::default();
        on_disk.storage.unlock_gnome_keyring = true;
        on_disk.storage.encrypt_templates = false;
        on_disk.liveness.enabled = true;

        let mut last_good = gaze_core::config::Config::default();
        let resolved = super::resolve_config(Ok(on_disk), &mut last_good);
        assert!(!resolved.storage.unlock_gnome_keyring);
        assert!(
            !last_good.storage.unlock_gnome_keyring,
            "the clamped value is what gets remembered"
        );

        let mut valid = gaze_core::config::Config::default();
        valid.storage.unlock_gnome_keyring = true;
        valid.storage.encrypt_templates = true;
        valid.liveness.enabled = true;
        let resolved = super::resolve_config(Ok(valid), &mut last_good);
        assert!(resolved.storage.unlock_gnome_keyring);
    }

    #[test]
    fn an_unreadable_config_keeps_the_last_good_one() {
        let mut last_good = hardened_config();

        let resolved = super::resolve_config(
            Err(anyhow::anyhow!("expected `=` after key, found newline")),
            &mut last_good,
        );

        assert_eq!(resolved.security.level, "maximum");
        assert!(resolved.auth.require_confirmation_lock_screen);
        assert!(resolved.auth.require_confirmation_elevation);
        assert_eq!(last_good.security.level, "maximum");
    }

    #[test]
    fn defaults_would_have_weakened_the_running_settings() {
        let defaults = gaze_core::config::Config::default();
        let hardened = hardened_config();

        assert_ne!(defaults.security.level, hardened.security.level);
        assert!(!defaults.auth.require_confirmation_lock_screen);
        assert!(!defaults.auth.require_confirmation_elevation);
    }

    #[test]
    fn a_readable_config_replaces_the_last_good_one() {
        let mut last_good = hardened_config();
        let mut updated = gaze_core::config::Config::default();
        updated.liveness.threshold = 0.95;

        let resolved = super::resolve_config(Ok(updated), &mut last_good);

        assert_eq!(resolved.liveness.threshold, 0.95);
        assert_eq!(last_good.liveness.threshold, 0.95);
        assert_eq!(last_good.security.level, "medium");
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
