// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

mod security;

pub use security::{CustomSecurityForm, SecurityField, SecurityLevel, SecurityValidationError};

pub const CONFIG_PATH: &str = "/etc/gaze/config.toml";
pub const USERS_DIR: &str = "/var/lib/gaze/users";
pub const MODELS_DIR: &str = "/var/cache/gaze";
pub const DEFAULT_RGB_CAMERA: &str = "primary";
pub const SECURITY_LEVEL_OPTIONS: [&str; 5] = ["low", "medium", "high", "maximum", "custom"];
pub const SECURITY_LEVEL_LABELS: [&str; 5] = ["Low", "Medium", "High", "Maximum", "Custom"];
pub const MODEL_QUALITY_OPTIONS: [&str; 2] = ["standard", "accurate"];
pub const MODEL_QUALITY_LABELS: [&str; 2] = ["Standard", "Accurate"];
pub const HYBRID_POLICY_OPTIONS: [&str; 4] = ["default", "or", "fallback_on_dark", "and"];
pub const HYBRID_POLICY_LABELS: [&str; 4] = ["Default", "Or", "Fallback on dark", "And"];
pub const PARALLEL_CAPTURE_OPTIONS: [&str; 3] = ["never", "auto", "always"];
pub const PARALLEL_CAPTURE_LABELS: [&str; 3] = [
    "Never (capture RGB, then IR)",
    "Auto (parallel when the camera allows it)",
    "Always (capture RGB and IR at once)",
];
pub const START_DELAY_SCOPE_OPTIONS: [&str; 2] = ["all", "screen_lock"];
pub const START_DELAY_SCOPE_LABELS: [&str; 2] = [
    "All face authentication (including sudo)",
    "Screen lockers only",
];
// Clients validate syntax independently of the daemon's installed vendor runtime.
pub const INFERENCE_EXECUTION_PROVIDER_OPTIONS: [&str; 4] = ["cpu", "auto", "openvino", "vitis"];
pub const INFERENCE_EXECUTION_PROVIDER_LABELS: [&str; 4] =
    ["CPU", "Automatic (NPU)", "Intel OpenVINO", "AMD Ryzen AI"];
pub const INFERENCE_DEVICE_OPTIONS: [&str; 3] = ["cpu", "gpu", "npu"];
pub const INFERENCE_DEVICE_LABELS: [&str; 3] = ["CPU", "GPU", "NPU"];
pub const DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO: f64 = 0.25;
pub const MIN_ENROLLMENT_FACE_SIZE_RATIO: f64 = 0.10;
pub const MAX_ENROLLMENT_FACE_SIZE_RATIO: f64 = 0.75;
pub const DEFAULT_SECURITY_THRESHOLD: f64 = 0.4;
pub const MIN_SECURITY_THRESHOLD: f64 = 0.10;
pub const MAX_SECURITY_THRESHOLD: f64 = 0.99;
pub const MIN_LIVENESS_THRESHOLD: f64 = 0.10;
pub const MAX_LIVENESS_THRESHOLD: f64 = 1.0;
pub const MIN_LIVENESS_MAX_SECONDS: f64 = 0.2;
pub const MAX_LIVENESS_MAX_SECONDS: f64 = 30.0;
pub const DEFAULT_LIVENESS_MAX_SECONDS: f64 = 2.0;
pub const MIN_LIVENESS_FRAMES: u32 = 6;
pub const DEFAULT_IR_LIVENESS_THRESHOLD: f64 = 0.5;
pub const DEFAULT_CAMERA_FPS: f64 = 30.0;
pub const DEFAULT_DURESS_CLOSED_THRESHOLD: f64 = 0.9;
pub const MIN_DURESS_CLOSED_THRESHOLD: f64 = 0.5;
pub const MAX_DURESS_CLOSED_THRESHOLD: f64 = 0.99;
pub const DEFAULT_DURESS_HOLD_MS: u64 = 600;
pub const MAX_DURESS_HOLD_MS: u64 = 5000;
pub const MAX_IR_FRAME_DIMENSION: u32 = 4096;
const DEFAULT_CONFIG_MODE: u32 = 0o644;

pub fn index_of(options: &[&str], value: &str, fallback: u32) -> u32 {
    options
        .iter()
        .position(|option| *option == value)
        .map(|index| index as u32)
        .unwrap_or(fallback)
}

pub fn value_at(options: &[&'static str], index: usize, fallback: &'static str) -> &'static str {
    options.get(index).copied().unwrap_or(fallback)
}

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub inference: InferenceConfig,
    #[serde(default)]
    pub security: SecurityLevel,
    #[serde(default)]
    pub cameras: CameraConfig,
    #[serde(default)]
    pub auth: AuthConfig,
    #[serde(default)]
    pub enrollment: EnrollmentConfig,
    #[serde(default)]
    pub liveness: LivenessConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub duress: DuressConfig,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct InferenceConfig {
    #[serde(default = "default_execution_provider")]
    pub execution_provider: String,
    #[serde(default = "default_inference_device")]
    pub device: String,
}

fn default_execution_provider() -> String {
    "cpu".to_string()
}

fn default_inference_device() -> String {
    "cpu".to_string()
}

impl Default for InferenceConfig {
    fn default() -> Self {
        Self {
            execution_provider: default_execution_provider(),
            device: default_inference_device(),
        }
    }
}

impl InferenceConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !INFERENCE_EXECUTION_PROVIDER_OPTIONS.contains(&self.execution_provider.as_str()) {
            anyhow::bail!(
                "invalid inference.execution_provider {:?}: expected one of {:?}",
                self.execution_provider,
                INFERENCE_EXECUTION_PROVIDER_OPTIONS
            );
        }
        if !INFERENCE_DEVICE_OPTIONS.contains(&self.device.as_str()) {
            anyhow::bail!(
                "invalid inference.device {:?}: expected one of {:?}",
                self.device,
                INFERENCE_DEVICE_OPTIONS
            );
        }
        if self.execution_provider == "cpu" && self.device != "cpu" {
            anyhow::bail!(
                "inference.device must be \"cpu\" when inference.execution_provider is \"cpu\""
            );
        }
        if matches!(self.execution_provider.as_str(), "auto" | "vitis") && self.device != "npu" {
            anyhow::bail!(
                "inference.device must be \"npu\" when inference.execution_provider is {:?}",
                self.execution_provider
            );
        }
        Ok(())
    }

    pub fn is_representable(&self) -> bool {
        INFERENCE_EXECUTION_PROVIDER_OPTIONS.contains(&self.execution_provider.as_str())
            && INFERENCE_DEVICE_OPTIONS.contains(&self.device.as_str())
    }

    pub fn execution_provider_index(&self) -> u32 {
        index_of(
            &INFERENCE_EXECUTION_PROVIDER_OPTIONS,
            &self.execution_provider,
            0,
        )
    }

    pub fn device_index(&self) -> u32 {
        index_of(&INFERENCE_DEVICE_OPTIONS, &self.device, 0)
    }

    pub fn execution_provider_from_index(index: usize) -> &'static str {
        value_at(&INFERENCE_EXECUTION_PROVIDER_OPTIONS, index, "cpu")
    }

    pub fn device_from_index(index: usize) -> &'static str {
        value_at(&INFERENCE_DEVICE_OPTIONS, index, "cpu")
    }
}

// Its own table: a security preset replaces `[security]` wholesale, resetting it.
#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct StorageConfig {
    #[serde(default = "default_true")]
    pub encrypt_templates: bool,
    #[serde(default = "default_false")]
    pub unlock_gnome_keyring: bool,
    #[serde(default = "default_false")]
    pub unlock_kwallet: bool,
}

impl Config {
    /// Disable keyring unlock when prerequisites are missing. Returns whether it was cleared.
    pub fn clamp_keyring(&mut self) -> bool {
        if self.storage.validate_keyring(&self.liveness).is_err() {
            self.storage.unlock_gnome_keyring = false;
            self.storage.unlock_kwallet = false;
            return true;
        }
        false
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            encrypt_templates: true,
            unlock_gnome_keyring: false,
            unlock_kwallet: false,
        }
    }
}

impl StorageConfig {
    pub fn validate_keyring(&self, liveness: &LivenessConfig) -> anyhow::Result<()> {
        if (self.unlock_gnome_keyring || self.unlock_kwallet)
            && (!self.encrypt_templates || !liveness.enabled)
        {
            anyhow::bail!("keyring unlock requires storage.encrypt_templates and liveness.enabled");
        }
        Ok(())
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DuressConfig {
    #[serde(default = "default_false")]
    pub enabled: bool,
    #[serde(default = "default_duress_closed_threshold")]
    pub closed_threshold: f64,
    #[serde(default = "default_duress_hold_ms")]
    pub hold_ms: u64,
}

fn default_duress_closed_threshold() -> f64 {
    DEFAULT_DURESS_CLOSED_THRESHOLD
}

fn default_duress_hold_ms() -> u64 {
    DEFAULT_DURESS_HOLD_MS
}

impl Default for DuressConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            closed_threshold: default_duress_closed_threshold(),
            hold_ms: default_duress_hold_ms(),
        }
    }
}

impl DuressConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !Self::closed_threshold_in_range(self.closed_threshold) {
            anyhow::bail!(
                "duress.closed_threshold must be between {} and {}, got {}",
                MIN_DURESS_CLOSED_THRESHOLD,
                MAX_DURESS_CLOSED_THRESHOLD,
                self.closed_threshold
            );
        }
        if self.hold_ms > MAX_DURESS_HOLD_MS {
            anyhow::bail!(
                "duress.hold_ms must be at most {}, got {}",
                MAX_DURESS_HOLD_MS,
                self.hold_ms
            );
        }
        Ok(())
    }

    fn closed_threshold_in_range(threshold: f64) -> bool {
        threshold.is_finite()
            && (MIN_DURESS_CLOSED_THRESHOLD..=MAX_DURESS_CLOSED_THRESHOLD).contains(&threshold)
    }

    pub fn effective_closed_threshold(&self) -> f32 {
        if Self::closed_threshold_in_range(self.closed_threshold) {
            self.closed_threshold as f32
        } else {
            DEFAULT_DURESS_CLOSED_THRESHOLD as f32
        }
    }

    pub fn effective_hold(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.hold_ms.min(MAX_DURESS_HOLD_MS))
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LivenessConfig {
    #[serde(default = "default_liveness_enabled")]
    pub enabled: bool,
    #[serde(default = "default_liveness_threshold")]
    pub threshold: f64,
    #[serde(default = "default_max_seconds")]
    pub max_seconds: f64,
    #[serde(default)]
    pub ir_model: bool,
    #[serde(default = "default_ir_liveness_threshold")]
    pub ir_threshold: f64,
}

fn default_liveness_enabled() -> bool {
    true
}
fn default_liveness_threshold() -> f64 {
    0.8
}
fn default_max_seconds() -> f64 {
    DEFAULT_LIVENESS_MAX_SECONDS
}
fn default_ir_liveness_threshold() -> f64 {
    DEFAULT_IR_LIVENESS_THRESHOLD
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            enabled: default_liveness_enabled(),
            threshold: default_liveness_threshold(),
            max_seconds: default_max_seconds(),
            ir_model: false,
            ir_threshold: default_ir_liveness_threshold(),
        }
    }
}

impl LivenessConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !Self::threshold_in_range(self.threshold) {
            anyhow::bail!(
                "liveness.threshold must be between {} and {}, got {}",
                MIN_LIVENESS_THRESHOLD,
                MAX_LIVENESS_THRESHOLD,
                self.threshold
            );
        }
        if !Self::max_seconds_in_range(self.max_seconds) {
            anyhow::bail!(
                "liveness.max_seconds must be between {} and {}, got {}",
                MIN_LIVENESS_MAX_SECONDS,
                MAX_LIVENESS_MAX_SECONDS,
                self.max_seconds
            );
        }
        if !Self::threshold_in_range(self.ir_threshold) {
            anyhow::bail!(
                "liveness.ir_threshold must be between {} and {}, got {}",
                MIN_LIVENESS_THRESHOLD,
                MAX_LIVENESS_THRESHOLD,
                self.ir_threshold
            );
        }
        Ok(())
    }

    fn threshold_in_range(threshold: f64) -> bool {
        threshold.is_finite()
            && (MIN_LIVENESS_THRESHOLD..=MAX_LIVENESS_THRESHOLD).contains(&threshold)
    }

    fn max_seconds_in_range(max_seconds: f64) -> bool {
        max_seconds.is_finite()
            && (MIN_LIVENESS_MAX_SECONDS..=MAX_LIVENESS_MAX_SECONDS).contains(&max_seconds)
    }

    pub fn effective_threshold(&self) -> f64 {
        if Self::threshold_in_range(self.threshold) {
            self.threshold
        } else {
            default_liveness_threshold()
        }
    }

    pub fn effective_ir_threshold(&self) -> f64 {
        if Self::threshold_in_range(self.ir_threshold) {
            self.ir_threshold
        } else {
            default_ir_liveness_threshold()
        }
    }

    pub fn ir_model_enabled(&self) -> bool {
        self.enabled && self.ir_model
    }

    pub fn effective_max_seconds(&self) -> f64 {
        if Self::max_seconds_in_range(self.max_seconds) {
            self.max_seconds
        } else {
            default_max_seconds()
        }
    }

    pub fn effective_max_frames(&self, fps: f64) -> u32 {
        let fps = if fps > 0.0 && fps.is_finite() {
            fps
        } else {
            DEFAULT_CAMERA_FPS
        };
        let frames = (self.effective_max_seconds() * fps).round() as u32;
        frames.max(MIN_LIVENESS_FRAMES)
    }
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CameraConfig {
    #[serde(default = "default_rgb_device")]
    pub rgb: String,
    #[serde(default)]
    pub ir: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir_frame_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ir_frame_height: Option<u32>,
    #[serde(default)]
    pub emitter_enabled: bool,
    #[serde(default = "default_dark_luma_threshold")]
    pub dark_luma_threshold: u8,
    #[serde(default = "default_parallel_capture")]
    pub parallel_capture: String,
}

fn default_rgb_device() -> String {
    DEFAULT_RGB_CAMERA.to_string()
}

fn ir_frame_dimension_valid(dimension: u32) -> bool {
    (1..=MAX_IR_FRAME_DIMENSION).contains(&dimension)
}

fn default_dark_luma_threshold() -> u8 {
    20
}

fn default_parallel_capture() -> String {
    "never".to_string()
}

impl CameraConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !PARALLEL_CAPTURE_OPTIONS.contains(&self.parallel_capture.as_str()) {
            anyhow::bail!(
                "invalid cameras.parallel_capture {:?}: expected one of {:?}",
                self.parallel_capture,
                PARALLEL_CAPTURE_OPTIONS
            );
        }
        match (self.ir_frame_width, self.ir_frame_height) {
            (None, None) => {}
            (Some(_), None) | (None, Some(_)) => anyhow::bail!(
                "cameras.ir_frame_width and cameras.ir_frame_height must be set together"
            ),
            (Some(width), Some(height)) => {
                if !ir_frame_dimension_valid(width) || !ir_frame_dimension_valid(height) {
                    anyhow::bail!(
                        "cameras.ir_frame_width and cameras.ir_frame_height must be between 1 and \
                         {MAX_IR_FRAME_DIMENSION}, got {width}x{height}"
                    );
                }
            }
        }
        Ok(())
    }

    pub fn ir_frame_size(&self) -> Option<(u32, u32)> {
        let width = self.ir_frame_width?;
        let height = self.ir_frame_height?;
        (ir_frame_dimension_valid(width) && ir_frame_dimension_valid(height))
            .then_some((width, height))
    }

    pub fn set_ir_frame_size(&mut self, size: Option<(u32, u32)>) {
        self.ir_frame_width = size.map(|(width, _)| width);
        self.ir_frame_height = size.map(|(_, height)| height);
    }

    pub fn parallel_capture(&self) -> &str {
        if PARALLEL_CAPTURE_OPTIONS.contains(&self.parallel_capture.as_str()) {
            &self.parallel_capture
        } else {
            "never"
        }
    }

    pub fn parallel_capture_index(&self) -> u32 {
        index_of(&PARALLEL_CAPTURE_OPTIONS, self.parallel_capture(), 0)
    }

    pub fn parallel_capture_from_index(index: usize) -> String {
        value_at(&PARALLEL_CAPTURE_OPTIONS, index, "never").to_string()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSurface {
    ScreenLock,
    Elevation,
    Login,
    /// No PAM prompt is involved, as with `gaze auth` or the GUI test button.
    /// These calls go straight to the daemon, so there is no locker to step away from.
    Direct,
}

const ELEVATION_SERVICES: [&str; 9] = [
    "sudo",
    "sudo-i",
    "su",
    "su-l",
    "doas",
    "run0",
    "systemd-run0",
    "polkit-1",
    "pkexec",
];

const LOGIN_SERVICES: [&str; 6] = [
    "login",
    "sddm",
    "lightdm",
    "greetd",
    "gdm-password",
    "gdm-launch-environment",
];

pub fn classify_pam_service(service: Option<&str>) -> AuthSurface {
    match service {
        None | Some("") => AuthSurface::Direct,
        Some(name) if ELEVATION_SERVICES.contains(&name) => AuthSurface::Elevation,
        Some(name) if LOGIN_SERVICES.contains(&name) => AuthSurface::Login,
        _ => AuthSurface::ScreenLock,
    }
}

fn default_start_delay_scope() -> String {
    "screen_lock".to_string()
}

#[derive(Serialize, Clone, Debug)]
pub struct AuthConfig {
    pub abort_if_ssh: bool,
    pub abort_if_lid_closed: bool,
    pub abort_before_first_resume: bool,
    pub require_confirmation_lock_screen: bool,
    pub require_confirmation_elevation: bool,
    pub resume_grace_ms: u64,
    pub start_delay_ms: u64,
    pub start_delay_scope: String,
}

#[derive(Deserialize)]
struct AuthConfigFile {
    #[serde(default = "default_true")]
    abort_if_ssh: bool,
    #[serde(default = "default_true")]
    abort_if_lid_closed: bool,
    #[serde(default = "default_false")]
    abort_before_first_resume: bool,
    #[serde(default)]
    require_confirmation: Option<bool>,
    #[serde(default)]
    require_confirmation_lock_screen: Option<bool>,
    #[serde(default)]
    require_confirmation_elevation: Option<bool>,
    #[serde(default = "default_resume_grace_ms")]
    resume_grace_ms: u64,
    #[serde(default = "default_start_delay_ms")]
    start_delay_ms: u64,
    #[serde(default = "default_start_delay_scope")]
    start_delay_scope: String,
}

impl<'de> Deserialize<'de> for AuthConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let file = AuthConfigFile::deserialize(deserializer)?;
        let legacy = file.require_confirmation.unwrap_or(false);

        Ok(Self {
            abort_if_ssh: file.abort_if_ssh,
            abort_if_lid_closed: file.abort_if_lid_closed,
            abort_before_first_resume: file.abort_before_first_resume,
            require_confirmation_lock_screen: file
                .require_confirmation_lock_screen
                .unwrap_or(legacy),
            require_confirmation_elevation: file.require_confirmation_elevation.unwrap_or(legacy),
            resume_grace_ms: file.resume_grace_ms,
            start_delay_ms: file.start_delay_ms,
            start_delay_scope: file.start_delay_scope,
        })
    }
}

fn default_false() -> bool {
    false
}

fn default_resume_grace_ms() -> u64 {
    0
}

fn default_start_delay_ms() -> u64 {
    0
}

impl AuthConfig {
    pub fn requires_confirmation(&self, surface: AuthSurface) -> bool {
        match surface {
            AuthSurface::Elevation => self.require_confirmation_elevation,
            AuthSurface::ScreenLock | AuthSurface::Login => self.require_confirmation_lock_screen,
            AuthSurface::Direct => false,
        }
    }

    pub fn start_delay_scope(&self) -> &str {
        match self.start_delay_scope.as_str() {
            "screen_lock" => "screen_lock",
            "" | "all" => "all",
            other => {
                tracing::warn!("Invalid start delay scope {other:?}; delaying every auth");
                "all"
            }
        }
    }

    fn start_delay_applies_to(&self, surface: AuthSurface) -> bool {
        if surface == AuthSurface::Direct {
            return false;
        }
        match self.start_delay_scope() {
            "screen_lock" => surface == AuthSurface::ScreenLock,
            _ => true,
        }
    }

    pub fn effective_start_delay_ms(&self, resumed: bool, surface: AuthSurface) -> u64 {
        self.start_delay_after_lock_ms(resumed, surface, None)
    }

    /// Milliseconds still owed on the start delay, measured from the lock.
    pub fn start_delay_after_lock_ms(
        &self,
        resumed: bool,
        surface: AuthSurface,
        lock_elapsed_ms: Option<u64>,
    ) -> u64 {
        let start = if self.start_delay_applies_to(surface) {
            match lock_elapsed_ms {
                Some(elapsed) => self.start_delay_ms.saturating_sub(elapsed),
                None => self.start_delay_ms,
            }
        } else {
            0
        };
        if resumed {
            start.max(self.resume_grace_ms)
        } else {
            start
        }
    }

    pub fn start_delay_scope_index_for_value(value: &str) -> u32 {
        index_of(&START_DELAY_SCOPE_OPTIONS, value, 0)
    }

    pub fn start_delay_scope_from_index(index: usize) -> String {
        value_at(&START_DELAY_SCOPE_OPTIONS, index, "all").to_string()
    }
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct EnrollmentConfig {
    #[serde(default = "default_max_templates")]
    pub max_templates: u32,
    #[serde(default = "default_enrollment_min_face_size_ratio")]
    pub min_face_size_ratio: f64,
}

fn default_max_templates() -> u32 {
    2
}

fn default_enrollment_min_face_size_ratio() -> f64 {
    DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO
}

impl EnrollmentConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        if !self.min_face_size_ratio.is_finite()
            || !(MIN_ENROLLMENT_FACE_SIZE_RATIO..=MAX_ENROLLMENT_FACE_SIZE_RATIO)
                .contains(&self.min_face_size_ratio)
        {
            anyhow::bail!(
                "enrollment.min_face_size_ratio must be between {} and {}, got {}",
                MIN_ENROLLMENT_FACE_SIZE_RATIO,
                MAX_ENROLLMENT_FACE_SIZE_RATIO,
                self.min_face_size_ratio
            );
        }
        Ok(())
    }

    pub fn effective_min_face_size_ratio(&self) -> f32 {
        if self.validate().is_ok() {
            self.min_face_size_ratio as f32
        } else {
            DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO as f32
        }
    }
}

impl Default for EnrollmentConfig {
    fn default() -> Self {
        Self {
            max_templates: default_max_templates(),
            min_face_size_ratio: default_enrollment_min_face_size_ratio(),
        }
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            abort_if_ssh: true,
            abort_if_lid_closed: true,
            abort_before_first_resume: false,
            require_confirmation_lock_screen: false,
            require_confirmation_elevation: false,
            resume_grace_ms: default_resume_grace_ms(),
            start_delay_ms: default_start_delay_ms(),
            start_delay_scope: default_start_delay_scope(),
        }
    }
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            rgb: default_rgb_device(),
            ir: String::new(),
            emitter_enabled: false,
            dark_luma_threshold: default_dark_luma_threshold(),
            parallel_capture: default_parallel_capture(),
            ir_frame_width: None,
            ir_frame_height: None,
        }
    }
}

const OPTIONAL_CONFIG_KEYS: [(&str, &str); 2] = [
    ("cameras", "ir_frame_width"),
    ("cameras", "ir_frame_height"),
];

const FILE_ONLY_CONFIG_KEYS: [&str; 2] = ["liveness.ir_model", "liveness.ir_threshold"];

const LEGACY_CONFIG_KEYS: [&str; 3] = [
    "security.threshold",
    "auth.require_confirmation",
    "liveness.max_frames",
];

pub fn migrate_legacy_config_contents(contents: &str) -> Option<String> {
    let mut doc: toml_edit::DocumentMut = contents.parse().ok()?;
    let liveness = doc.get_mut("liveness")?.as_table_mut()?;
    if let Some(val) = liveness.remove("max_frames") {
        let frames = val.as_value()?.as_integer()?;
        let seconds = if frames == 40 {
            DEFAULT_LIVENESS_MAX_SECONDS
        } else {
            ((frames as f64 / DEFAULT_CAMERA_FPS) * 100.0).round() / 100.0
        };
        liveness.insert("max_seconds", toml_edit::value(seconds));
        Some(doc.to_string())
    } else {
        None
    }
}

pub fn unknown_config_keys(contents: &str) -> Vec<String> {
    let Ok(known_doc) = toml_edit::ser::to_document(&Config::default()) else {
        return Vec::new();
    };
    let Ok(actual_doc) = contents.parse::<toml_edit::DocumentMut>() else {
        return Vec::new();
    };

    let mut unknown = Vec::new();
    for (table_name, table_item) in actual_doc.as_table().iter() {
        let Some(known_sub_item) = known_doc.as_table().get(table_name) else {
            unknown.push(table_name.to_string());
            continue;
        };
        let Some(actual_table) = table_item.as_table() else {
            continue;
        };
        for (key, _) in actual_table.iter() {
            let is_known = match known_sub_item {
                toml_edit::Item::Table(t) => t.contains_key(key),
                toml_edit::Item::Value(toml_edit::Value::InlineTable(t)) => t.contains_key(key),
                _ => false,
            };
            let path = format!("{table_name}.{key}");
            let is_optional = OPTIONAL_CONFIG_KEYS.contains(&(table_name, key));
            if !is_known && !is_optional && !LEGACY_CONFIG_KEYS.contains(&path.as_str()) {
                unknown.push(path);
            }
        }
    }
    unknown.sort();
    unknown
}

fn replace_file_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("config path must have a parent directory"))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| std::io::Error::other("config path must have a valid file name"))?;
    let mode = std::fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o777)
        .unwrap_or(DEFAULT_CONFIG_MODE);

    let tmp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    if let Err(e) = file
        .write_all(contents.as_bytes())
        .and_then(|_| file.flush())
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    drop(file);
    if let Err(e) = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path)
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(CONFIG_PATH)
    }

    pub fn to_toml(&self) -> anyhow::Result<String> {
        Ok(toml_edit::ser::to_string_pretty(self)?)
    }

    pub fn get_key(&self, key: &str) -> anyhow::Result<String> {
        let doc = toml_edit::ser::to_document(self)?;
        let mut item = doc.as_item();
        for part in key.split('.') {
            item = item
                .get(part)
                .ok_or_else(|| anyhow::anyhow!("unknown setting `{key}`"))?;
        }
        let value = item.as_value().filter(|_| !item.is_table_like());
        let Some(value) = value else {
            anyhow::bail!(
                "`{key}` is a section; ask for one of its settings, such as `{key}.<name>`"
            );
        };
        Ok(match value.as_str() {
            Some(text) => text.to_string(),
            None => value.to_string().trim().to_string(),
        })
    }

    pub fn with_key(&self, key: &str, raw: &str) -> anyhow::Result<Config> {
        let (section, leaf) = key
            .rsplit_once('.')
            .ok_or_else(|| anyhow::anyhow!("use section.setting, for example `security.level`"))?;
        if FILE_ONLY_CONFIG_KEYS.contains(&key) {
            anyhow::bail!("`{key}` can only be set in {CONFIG_PATH}; edit it and restart gazed");
        }
        let mut doc = toml_edit::ser::to_document(self)?;
        let mut table = doc.as_item_mut();
        for part in section.split('.') {
            table = table
                .get_mut(part)
                .ok_or_else(|| anyhow::anyhow!("unknown setting `{key}`"))?;
        }
        let table = table
            .as_table_like_mut()
            .ok_or_else(|| anyhow::anyhow!("unknown setting `{key}`"))?;
        let current = table
            .get(leaf)
            .and_then(toml_edit::Item::as_value)
            .ok_or_else(|| anyhow::anyhow!("unknown setting `{key}`"))?;
        let raw = raw.trim();
        let value = match current {
            toml_edit::Value::Boolean(_) => toml_edit::value(
                raw.parse::<bool>()
                    .map_err(|_| anyhow::anyhow!("`{key}` takes true or false"))?,
            ),
            toml_edit::Value::Integer(_) => toml_edit::value(
                raw.parse::<i64>()
                    .map_err(|_| anyhow::anyhow!("`{key}` takes a whole number"))?,
            ),
            toml_edit::Value::Float(_) => toml_edit::value(
                raw.parse::<f64>()
                    .map_err(|_| anyhow::anyhow!("`{key}` takes a number"))?,
            ),
            toml_edit::Value::String(_) => toml_edit::value(raw),
            _ => anyhow::bail!("`{key}` cannot be set from the command line; edit {CONFIG_PATH}"),
        };
        table.insert(leaf, value);
        let config: Config = toml_edit::de::from_document(doc)
            .map_err(|e| anyhow::anyhow!("invalid value for `{key}`: {e}"))?;
        config.security.validate()?;
        config.enrollment.validate()?;
        config.inference.validate()?;
        config.liveness.validate()?;
        config.cameras.validate()?;
        config.duress.validate()?;
        Ok(config)
    }

    pub fn migrate_file(path: &str) {
        let Ok(contents) = std::fs::read_to_string(path) else {
            return;
        };
        let Some(migrated) = migrate_legacy_config_contents(&contents) else {
            return;
        };
        match replace_file_atomically(Path::new(path), &migrated) {
            Ok(()) => tracing::info!("Migrated legacy configuration in {path}"),
            Err(e) => tracing::warn!("Failed to write migrated config to {path}: {e}"),
        }
    }

    pub fn load_from(path: &str) -> anyhow::Result<Self> {
        if Path::new(path).exists() {
            let contents = std::fs::read_to_string(path)?;
            let contents = migrate_legacy_config_contents(&contents).unwrap_or(contents);
            let config: Config = toml_edit::de::from_str(&contents)?;
            for key in unknown_config_keys(&contents) {
                tracing::warn!(
                    "{path} sets {key}, which Gaze does not read; it has no effect and is probably a typo"
                );
            }
            // Keep startup available for an invalid level: warn and let the total
            // accessors fall back. `set_config` rejects invalid admin input earlier.
            if let Err(e) = config.security.validate() {
                tracing::warn!("{e}; using safe fallbacks for invalid security fields");
            }
            if let Err(e) = config.enrollment.validate() {
                tracing::warn!("{e}; using the default enrollment face-size ratio");
            }
            if let Err(e) = config.inference.validate() {
                tracing::warn!("{e}; inference configuration will be checked when models load");
            }
            if let Err(e) = config.liveness.validate() {
                tracing::warn!("{e}; using the default liveness settings");
            }
            if let Err(e) = config.cameras.validate() {
                tracing::warn!("{e}; using the default for the invalid camera setting");
            }
            if let Err(e) = config.duress.validate() {
                tracing::warn!("{e}; using the default duress settings");
            }
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(CONFIG_PATH)
    }

    pub fn save_to(&self, path: &str) -> anyhow::Result<()> {
        let path = Path::new(path);
        let existing = std::fs::read_to_string(path).ok();
        let existing_mode = std::fs::metadata(path)
            .ok()
            .map(|meta| meta.permissions().mode() & 0o777);

        let encoded = self
            .rewrite_preserving_comments(existing.as_deref())
            .unwrap_or_else(|| {
                toml_edit::ser::to_string_pretty(self)
                    .unwrap_or_else(|_| String::from("# unwritable\n"))
            });
        let parent = path
            .parent()
            .context("config path must have a parent directory")?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .context("config path must have a valid file name")?;
        let tmp_path = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp_path)
            .with_context(|| {
                format!(
                    "failed to create temporary config file: {}",
                    tmp_path.display()
                )
            })?;
        if let Err(err) = file
            .write_all(encoded.as_bytes())
            .and_then(|_| file.flush())
        {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(err)
                .with_context(|| format!("failed to write config file: {}", path.display()));
        }
        drop(file);
        let target_mode = existing_mode.unwrap_or(DEFAULT_CONFIG_MODE);
        if let Err(err) =
            std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(target_mode))
        {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(err).with_context(|| {
                format!("failed to set mode {target_mode:o} on {}", path.display())
            });
        }
        std::fs::rename(&tmp_path, path)
            .with_context(|| format!("failed to replace config file: {}", path.display()))?;
        Ok(())
    }

    fn rewrite_preserving_comments(&self, existing: Option<&str>) -> Option<String> {
        let mut doc = existing?.parse::<toml_edit::DocumentMut>().ok()?;
        let desired_doc = toml_edit::ser::to_document(self).ok()?;
        if !merge_into_toml_table(doc.as_table_mut(), desired_doc.as_table()) {
            return None;
        }
        for (table, key) in OPTIONAL_CONFIG_KEYS {
            let unset = desired_doc
                .get(table)
                .and_then(|item| item.as_table_like())
                .is_none_or(|desired| !desired.contains_key(key));
            if unset
                && let Some(target) = doc.get_mut(table).and_then(|item| item.as_table_like_mut())
            {
                target.remove(key);
            }
        }
        Some(doc.to_string())
    }
}

fn merge_table_like(target: &mut toml_edit::Table, key: &str, val: &toml_edit::Value) -> bool {
    match val {
        toml_edit::Value::InlineTable(sub_inline) => {
            if !target.contains_key(key) {
                target.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let Some(sub_target) = target.get_mut(key).and_then(|item| item.as_table_mut()) else {
                return false;
            };
            for (sub_key, sub_val) in sub_inline.iter() {
                if !merge_table_like(sub_target, sub_key, sub_val) {
                    return false;
                }
            }
        }
        scalar => match target.get_mut(key).and_then(|item| item.as_value_mut()) {
            Some(slot) => {
                let decor = slot.decor().clone();
                *slot = scalar.clone();
                *slot.decor_mut() = decor;
            }
            None => {
                target.insert(key, toml_edit::value(scalar.clone()));
            }
        },
    }
    true
}

fn merge_into_toml_table(target: &mut toml_edit::Table, desired: &toml_edit::Table) -> bool {
    for (key, value) in desired.iter() {
        match value {
            toml_edit::Item::Table(sub_table) => {
                if !target.contains_key(key) {
                    target.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
                }
                let Some(sub_target) = target.get_mut(key).and_then(|item| item.as_table_mut())
                else {
                    return false;
                };
                if !merge_into_toml_table(sub_target, sub_table) {
                    return false;
                }
            }
            toml_edit::Item::Value(val) => {
                if !merge_table_like(target, key, val) {
                    return false;
                }
            }
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests;
