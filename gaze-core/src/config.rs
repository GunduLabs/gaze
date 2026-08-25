// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use zvariant::{OwnedValue, Type, Value};

pub const CONFIG_PATH: &str = "/etc/gaze/config.toml";
pub const USERS_DIR: &str = "/var/lib/gaze/users";
pub const MODELS_DIR: &str = "/var/cache/gaze";
pub const DEFAULT_RGB_CAMERA: &str = "primary";
pub const SECURITY_LEVEL_OPTIONS: [&str; 5] = ["low", "medium", "high", "maximum", "custom"];
pub const SECURITY_LEVEL_LABELS: [&str; 5] = ["Low", "Medium", "High", "Maximum", "Custom"];
pub const MODEL_QUALITY_OPTIONS: [&str; 2] = ["standard", "accurate"];
pub const MODEL_QUALITY_LABELS: [&str; 2] = ["Standard", "Accurate"];
pub const HYBRID_POLICY_OPTIONS: [&str; 4] = ["default", "or", "fallback_on_dark", "and"];
pub const HYBRID_POLICY_LABELS: [&str; 4] = ["Default", "Or", "Fallback on Dark", "And"];
pub const PARALLEL_CAPTURE_OPTIONS: [&str; 3] = ["never", "auto", "always"];
pub const PARALLEL_CAPTURE_LABELS: [&str; 3] = [
    "Never (capture RGB, then IR)",
    "Auto (parallel when the camera allows it)",
    "Always (capture RGB and IR at once)",
];
pub const START_DELAY_SCOPE_OPTIONS: [&str; 2] = ["all", "screen_lock"];
pub const START_DELAY_SCOPE_LABELS: [&str; 2] =
    ["Every face auth (including sudo)", "Screen lockers only"];
#[cfg(not(feature = "openvino-config"))]
pub const INFERENCE_EXECUTION_PROVIDER_OPTIONS: [&str; 1] = ["cpu"];
#[cfg(feature = "openvino-config")]
pub const INFERENCE_EXECUTION_PROVIDER_OPTIONS: [&str; 2] = ["cpu", "openvino"];
#[cfg(not(feature = "openvino-config"))]
pub const INFERENCE_DEVICE_OPTIONS: [&str; 1] = ["cpu"];
#[cfg(feature = "openvino-config")]
pub const INFERENCE_DEVICE_OPTIONS: [&str; 3] = ["cpu", "gpu", "npu"];
pub const DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO: f64 = 0.25;
pub const MIN_ENROLLMENT_FACE_SIZE_RATIO: f64 = 0.10;
pub const MAX_ENROLLMENT_FACE_SIZE_RATIO: f64 = 0.75;
pub const DEFAULT_SECURITY_THRESHOLD: f64 = 0.4;
pub const MIN_SECURITY_THRESHOLD: f64 = 0.10;
pub const MAX_SECURITY_THRESHOLD: f64 = 0.99;
pub const MIN_LIVENESS_THRESHOLD: f64 = 0.10;
pub const MAX_LIVENESS_THRESHOLD: f64 = 1.0;
pub const MIN_LIVENESS_MAX_FRAMES: u32 = 6;
const DEFAULT_CONFIG_MODE: u32 = 0o644;

fn default_level() -> String {
    "medium".to_string()
}

/// Which part of `[security]` a validation error came from, so a caller can
/// offer the fix that matches instead of one message for the whole table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityField {
    Level,
    ModelQuality,
    Threshold,
    HybridPolicy,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SecurityValidationError {
    pub field: SecurityField,
    pub message: String,
}

const fn str_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        if a[i] != b[i] {
            return false;
        }
        i += 1;
    }
    true
}

const fn const_index_of(options: &[&str], value: &str) -> u32 {
    let mut i = 0;
    while i < options.len() {
        if str_eq(options[i], value) {
            return i as u32;
        }
        i += 1;
    }
    panic!("option is not present in its own option list")
}

/// Index of `value` in `options`, or `fallback` when it is not listed.
pub fn index_of(options: &[&str], value: &str, fallback: u32) -> u32 {
    options
        .iter()
        .position(|option| *option == value)
        .map(|index| index as u32)
        .unwrap_or(fallback)
}

/// Value at `index` in `options`, or `fallback` when the index is out of range.
pub fn value_at(options: &[&'static str], index: usize, fallback: &'static str) -> &'static str {
    options.get(index).copied().unwrap_or(fallback)
}

/// The values the custom security rows should show for a given level: either the
/// stored custom values, or the values the selected preset actually resolves to.
#[derive(Clone, Debug, PartialEq)]
pub struct CustomSecurityForm {
    pub detector: String,
    pub recognizer: String,
    pub rgb_threshold: f64,
    pub ir_threshold: f64,
    pub hybrid_policy: String,
}

// Presets carry an f32 threshold; round so a prompt shows 0.4, not 0.4000000059604645.
fn round_threshold(threshold: f32) -> f64 {
    (threshold as f64 * 100.0).round() / 100.0
}

#[derive(Serialize, Clone, Debug, Value, OwnedValue, Type)]
pub struct SecurityLevel {
    pub level: String,
    pub detector: String,
    pub recognizer: String,
    pub rgb_threshold: f64,
    pub ir_threshold: f64,
    pub hybrid_policy: String,
}

#[derive(Deserialize)]
struct SecurityLevelFile {
    #[serde(default = "default_level")]
    level: String,
    #[serde(default)]
    detector: String,
    #[serde(default)]
    recognizer: String,
    #[serde(default)]
    threshold: Option<f64>,
    #[serde(default)]
    rgb_threshold: Option<f64>,
    #[serde(default)]
    ir_threshold: Option<f64>,
    #[serde(default)]
    hybrid_policy: String,
}

impl<'de> Deserialize<'de> for SecurityLevel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let file = SecurityLevelFile::deserialize(deserializer)?;
        let fallback_threshold = file.threshold.unwrap_or(0.0);

        Ok(Self {
            level: file.level,
            detector: file.detector,
            recognizer: file.recognizer,
            rgb_threshold: file.rgb_threshold.unwrap_or(fallback_threshold),
            ir_threshold: file.ir_threshold.unwrap_or(fallback_threshold),
            hybrid_policy: file.hybrid_policy,
        })
    }
}

impl Default for SecurityLevel {
    fn default() -> Self {
        Self::preset(&default_level())
    }
}

impl SecurityLevel {
    pub const CUSTOM_LEVEL_INDEX: u32 = const_index_of(&SECURITY_LEVEL_OPTIONS, "custom");

    fn preset(level: &str) -> Self {
        Self {
            level: level.to_string(),
            detector: String::new(),
            recognizer: String::new(),
            rgb_threshold: 0.0,
            ir_threshold: 0.0,
            hybrid_policy: String::new(),
        }
    }

    pub fn low() -> Self {
        Self::preset("low")
    }

    pub fn medium() -> Self {
        Self::preset("medium")
    }

    pub fn high() -> Self {
        Self::preset("high")
    }

    pub fn maximum() -> Self {
        Self::preset("maximum")
    }

    pub fn custom(
        detector: String,
        recognizer: String,
        threshold: f64,
        hybrid_policy: String,
    ) -> Self {
        Self::custom_with_thresholds(detector, recognizer, threshold, threshold, hybrid_policy)
    }

    pub fn custom_with_thresholds(
        detector: String,
        recognizer: String,
        rgb_threshold: f64,
        ir_threshold: f64,
        hybrid_policy: String,
    ) -> Self {
        Self {
            level: "custom".to_string(),
            detector,
            recognizer,
            rgb_threshold,
            ir_threshold,
            hybrid_policy,
        }
    }

    pub fn preset_from_index(index: usize) -> Option<Self> {
        match index {
            0 => Some(Self::low()),
            1 => Some(Self::medium()),
            2 => Some(Self::high()),
            3 => Some(Self::maximum()),
            _ => None,
        }
    }

    pub fn level_index(&self) -> u32 {
        index_of(&SECURITY_LEVEL_OPTIONS, &self.level, 1)
    }

    pub fn model_quality_index(value: &str) -> u32 {
        index_of(&MODEL_QUALITY_OPTIONS, value, 0)
    }

    pub fn model_quality_from_index(index: usize) -> &'static str {
        value_at(&MODEL_QUALITY_OPTIONS, index, "standard")
    }

    pub fn hybrid_policy_index_for_value(value: &str) -> u32 {
        index_of(&HYBRID_POLICY_OPTIONS, value, 0)
    }

    pub fn hybrid_policy_from_index(index: usize) -> String {
        value_at(&HYBRID_POLICY_OPTIONS, index, "default").to_string()
    }

    /// Seeds the custom rows so switching a preset to "custom" starts from what
    /// the preset resolved to, instead of resetting to an unrelated default.
    pub fn custom_form(&self) -> CustomSecurityForm {
        if self.level == "custom" {
            CustomSecurityForm {
                detector: self.detector.clone(),
                recognizer: self.recognizer.clone(),
                rgb_threshold: self.rgb_threshold,
                ir_threshold: self.ir_threshold,
                hybrid_policy: self.hybrid_policy.clone(),
            }
        } else {
            CustomSecurityForm {
                detector: self.effective_detector_quality().to_string(),
                recognizer: self.effective_recognizer_quality().to_string(),
                rgb_threshold: round_threshold(self.rgb_threshold()),
                ir_threshold: round_threshold(self.ir_threshold()),
                hybrid_policy: self.hybrid_policy().to_string(),
            }
        }
    }

    // Accessors are total: an unknown level falls back to the medium models rather
    // than panicking the daemon. Bad input is rejected up front by `validate()`.
    pub fn detector(&self) -> &str {
        match self.level.as_str() {
            "low" | "medium" => "det_500m.onnx",
            "high" | "maximum" => "det_10g.onnx",
            "custom" => match self.detector.as_str() {
                "accurate" => "det_10g.onnx",
                _ => "det_500m.onnx",
            },
            other => {
                tracing::warn!("invalid security level {other:?}; using medium detector");
                "det_500m.onnx"
            }
        }
    }

    pub fn recognizer(&self) -> &str {
        match self.level.as_str() {
            "low" | "medium" => "w600k_mbf.onnx",
            "high" | "maximum" => "w600k_r50.onnx",
            "custom" => match self.recognizer.as_str() {
                "accurate" => "w600k_r50.onnx",
                _ => "w600k_mbf.onnx",
            },
            other => {
                tracing::warn!("invalid security level {other:?}; using medium recognizer");
                "w600k_mbf.onnx"
            }
        }
    }

    // The quality tier a level actually resolves to. Derived from the model accessors so a
    // UI pre-filling the custom rows from a preset can never drift from what the daemon loads.
    pub fn effective_detector_quality(&self) -> &'static str {
        match self.detector() {
            "det_10g.onnx" => "accurate",
            _ => "standard",
        }
    }

    pub fn effective_recognizer_quality(&self) -> &'static str {
        match self.recognizer() {
            "w600k_r50.onnx" => "accurate",
            _ => "standard",
        }
    }

    /// Every problem with the current values. `validate` reports only the first,
    /// so a report that wants to list them all must use this instead.
    pub fn validation_errors(&self) -> Vec<SecurityValidationError> {
        let mut errors = Vec::new();
        if !SECURITY_LEVEL_OPTIONS.contains(&self.level.as_str()) {
            errors.push(SecurityValidationError {
                field: SecurityField::Level,
                message: format!(
                    "invalid security level {:?}: expected one of {:?}",
                    self.level, SECURITY_LEVEL_OPTIONS
                ),
            });
            return errors;
        }
        if self.level != "custom" {
            return errors;
        }

        for (name, quality) in [
            ("detector", self.detector.as_str()),
            ("recognizer", self.recognizer.as_str()),
        ] {
            if !MODEL_QUALITY_OPTIONS.contains(&quality) {
                errors.push(SecurityValidationError {
                    field: SecurityField::ModelQuality,
                    message: format!(
                        "invalid {name} level {quality:?}: expected \"standard\" or \"accurate\""
                    ),
                });
            }
        }
        for (name, threshold) in [
            ("rgb_threshold", self.rgb_threshold),
            ("ir_threshold", self.ir_threshold),
        ] {
            if !Self::threshold_in_range(threshold) {
                errors.push(SecurityValidationError {
                    field: SecurityField::Threshold,
                    message: format!(
                        "security.{name} must be between {MIN_SECURITY_THRESHOLD} and {MAX_SECURITY_THRESHOLD}, got {threshold}"
                    ),
                });
            }
        }
        if !self.hybrid_policy.is_empty()
            && !HYBRID_POLICY_OPTIONS.contains(&self.hybrid_policy.as_str())
        {
            errors.push(SecurityValidationError {
                field: SecurityField::HybridPolicy,
                message: format!(
                    "invalid security.hybrid_policy {:?}: expected one of {:?}",
                    self.hybrid_policy, HYBRID_POLICY_OPTIONS
                ),
            });
        }
        errors
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        match self.validation_errors().into_iter().next() {
            Some(err) => Err(anyhow::anyhow!(err.message)),
            None => Ok(()),
        }
    }

    fn threshold_in_range(threshold: f64) -> bool {
        threshold.is_finite()
            && (MIN_SECURITY_THRESHOLD..=MAX_SECURITY_THRESHOLD).contains(&threshold)
    }

    /// Back-compat alias resolving to the **RGB** threshold. Prefer [`Self::rgb_threshold`] or
    /// [`Self::ir_threshold`], since an IR path calling this silently matches the RGB value.
    pub fn threshold(&self) -> f32 {
        self.rgb_threshold()
    }

    pub fn rgb_threshold(&self) -> f32 {
        self.threshold_for("rgb", self.rgb_threshold)
    }

    pub fn ir_threshold(&self) -> f32 {
        self.threshold_for("ir", self.ir_threshold)
    }

    fn threshold_for(&self, spectrum: &str, custom_threshold: f64) -> f32 {
        match self.level.as_str() {
            "low" => 0.3,
            "medium" => 0.4,
            "high" => 0.5,
            "maximum" => 0.6,
            "custom" if Self::threshold_in_range(custom_threshold) => custom_threshold as f32,
            "custom" => {
                tracing::warn!(
                    spectrum,
                    threshold = custom_threshold,
                    "security threshold is out of range; using the medium default"
                );
                DEFAULT_SECURITY_THRESHOLD as f32
            }
            _ => 0.4,
        }
    }

    pub fn hybrid_policy(&self) -> &str {
        match self.level.as_str() {
            "low" => "or",
            "medium" | "high" => "fallback_on_dark",
            "maximum" => "and",
            "custom" => {
                if self.hybrid_policy.is_empty() {
                    "fallback_on_dark"
                } else {
                    &self.hybrid_policy
                }
            }
            _ => "fallback_on_dark",
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, Value, OwnedValue, Type)]
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
}

#[derive(Deserialize, Serialize, Clone, Debug, Value, OwnedValue, Type)]
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
        #[cfg(not(feature = "openvino-config"))]
        if self.execution_provider == "openvino" {
            anyhow::bail!(
                "this Gaze build does not include OpenVINO support; rebuild with the \"openvino\" Cargo feature"
            );
        }
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
#[derive(Deserialize, Serialize, Clone, Debug, Default, Value, OwnedValue, Type)]
pub struct StorageConfig {
    #[serde(default = "default_false")]
    pub encrypt_templates: bool,
}

#[derive(Deserialize, Serialize, Clone, Debug, Value, OwnedValue, Type)]
pub struct LivenessConfig {
    #[serde(default = "default_liveness_enabled")]
    pub enabled: bool,
    #[serde(default = "default_liveness_threshold")]
    pub threshold: f64,
    #[serde(default = "default_max_frames")]
    pub max_frames: u32,
}

fn default_liveness_enabled() -> bool {
    true
}
fn default_liveness_threshold() -> f64 {
    0.8
}
fn default_max_frames() -> u32 {
    40
}

impl Default for LivenessConfig {
    fn default() -> Self {
        Self {
            enabled: default_liveness_enabled(),
            threshold: default_liveness_threshold(),
            max_frames: default_max_frames(),
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
        if self.max_frames < MIN_LIVENESS_MAX_FRAMES {
            anyhow::bail!(
                "liveness.max_frames must be at least {}, got {}",
                MIN_LIVENESS_MAX_FRAMES,
                self.max_frames
            );
        }
        Ok(())
    }

    fn threshold_in_range(threshold: f64) -> bool {
        threshold.is_finite()
            && (MIN_LIVENESS_THRESHOLD..=MAX_LIVENESS_THRESHOLD).contains(&threshold)
    }

    pub fn effective_threshold(&self) -> f64 {
        if Self::threshold_in_range(self.threshold) {
            self.threshold
        } else {
            default_liveness_threshold()
        }
    }

    pub fn effective_max_frames(&self) -> u32 {
        if self.max_frames >= MIN_LIVENESS_MAX_FRAMES {
            self.max_frames
        } else {
            default_max_frames()
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Value, OwnedValue, Type)]
pub struct CameraConfig {
    #[serde(default = "default_rgb_device")]
    pub rgb: String,
    #[serde(default)]
    pub ir: String,
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
        Ok(())
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
    /// No PAM prompt at all: `gaze auth`, the GUI test button, anything that
    /// calls the daemon directly. There is no locker to step away from.
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

#[derive(Serialize, Clone, Debug, Value, OwnedValue, Type)]
pub struct AuthConfig {
    pub abort_if_ssh: bool,
    pub abort_if_lid_closed: bool,
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
                tracing::warn!("invalid start delay scope {other:?}; delaying every auth");
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

    /// Milliseconds to wait before face verification begins.
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

#[derive(Deserialize, Serialize, Clone, Debug, Value, OwnedValue, Type)]
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
        }
    }
}

const LEGACY_CONFIG_KEYS: [&str; 2] = ["security.threshold", "auth.require_confirmation"];

pub fn unknown_config_keys(contents: &str) -> Vec<String> {
    let Ok(toml::Value::Table(known)) = toml::Value::try_from(Config::default()) else {
        return Vec::new();
    };
    let Ok(actual) = contents.parse::<toml::Table>() else {
        return Vec::new();
    };

    let mut unknown = Vec::new();
    for (table_name, table_value) in &actual {
        let Some(toml::Value::Table(known_table)) = known.get(table_name) else {
            unknown.push(table_name.clone());
            continue;
        };
        let Some(actual_table) = table_value.as_table() else {
            continue;
        };
        for key in actual_table.keys() {
            let path = format!("{table_name}.{key}");
            if !known_table.contains_key(key) && !LEGACY_CONFIG_KEYS.contains(&path.as_str()) {
                unknown.push(path);
            }
        }
    }
    unknown
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(CONFIG_PATH)
    }

    pub fn load_from(path: &str) -> anyhow::Result<Self> {
        if Path::new(path).exists() {
            let contents = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&contents)?;
            for key in unknown_config_keys(&contents) {
                tracing::warn!(
                    "{path} sets {key}, which Gaze does not read; it has no effect and is probably a typo"
                );
            }
            // Don't refuse to start on a bad level: warn and let the total accessors
            // fall back. Rejection is enforced at the set_config (admin input) boundary.
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
                tracing::warn!("{e}; capturing RGB and IR one at a time");
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
                toml::to_string_pretty(self).unwrap_or_else(|_| String::from("# unwritable\n"))
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
        let toml::Value::Table(desired) = toml::Value::try_from(self).ok()? else {
            return None;
        };
        merge_into_toml_table(doc.as_table_mut(), &desired).then(|| doc.to_string())
    }
}

fn toml_value_to_edit(value: &toml::Value) -> Option<toml_edit::Value> {
    Some(match value {
        toml::Value::String(v) => v.clone().into(),
        toml::Value::Integer(v) => (*v).into(),
        toml::Value::Float(v) => (*v).into(),
        toml::Value::Boolean(v) => (*v).into(),
        _ => return None,
    })
}

fn merge_into_toml_table(target: &mut toml_edit::Table, desired: &toml::Table) -> bool {
    for (key, value) in desired {
        match value {
            toml::Value::Table(sub) => {
                if !target.contains_key(key) {
                    target.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
                }
                let Some(sub_target) = target.get_mut(key).and_then(|item| item.as_table_mut())
                else {
                    return false;
                };
                if !merge_into_toml_table(sub_target, sub) {
                    return false;
                }
            }
            scalar => {
                let Some(new_value) = toml_value_to_edit(scalar) else {
                    return false;
                };
                match target.get_mut(key).and_then(|item| item.as_value_mut()) {
                    Some(slot) => {
                        let decor = slot.decor().clone();
                        *slot = new_value;
                        *slot.decor_mut() = decor;
                    }
                    None => {
                        target.insert(key, toml_edit::value(new_value));
                    }
                }
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "gaze-config-test-{}-{}-{name}",
                std::process::id(),
                unique
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn security_level_mappings_are_stable() {
        let cases = [
            (SecurityLevel::low(), "det_500m.onnx", "w600k_mbf.onnx", 0.3),
            (
                SecurityLevel::medium(),
                "det_500m.onnx",
                "w600k_mbf.onnx",
                0.4,
            ),
            (SecurityLevel::high(), "det_10g.onnx", "w600k_r50.onnx", 0.5),
            (
                SecurityLevel::maximum(),
                "det_10g.onnx",
                "w600k_r50.onnx",
                0.6,
            ),
        ];

        for (level, detector, recognizer, threshold) in cases {
            assert_eq!(level.detector(), detector);
            assert_eq!(level.recognizer(), recognizer);
            assert!((level.threshold() - threshold).abs() < f32::EPSILON);
            assert!((level.ir_threshold() - threshold).abs() < f32::EPSILON);
        }

        let custom = SecurityLevel::custom(
            "accurate".to_string(),
            "accurate".to_string(),
            0.73,
            String::new(),
        );
        assert_eq!(custom.detector(), "det_10g.onnx");
        assert_eq!(custom.recognizer(), "w600k_r50.onnx");
        assert!((custom.threshold() - 0.73).abs() < f32::EPSILON);
        assert!((custom.ir_threshold() - 0.73).abs() < f32::EPSILON);

        let custom_standard = SecurityLevel::custom(
            "standard".to_string(),
            "standard".to_string(),
            0.35,
            String::new(),
        );
        assert_eq!(custom_standard.detector(), "det_500m.onnx");
        assert_eq!(custom_standard.recognizer(), "w600k_mbf.onnx");
        assert!((custom_standard.threshold() - 0.35).abs() < f32::EPSILON);
    }

    #[test]
    fn validate_rejects_out_of_range_custom_threshold() {
        for bad in [-1.0, -0.5, 0.0, 0.09, 1.01, 5.0, f64::NAN, f64::INFINITY] {
            let level = SecurityLevel::custom(
                "standard".to_string(),
                "standard".to_string(),
                bad,
                String::new(),
            );
            assert!(level.validate().is_err(), "{bad}");
        }

        for bad in [-1.0, -0.5, 0.0, 0.09, 1.01, 5.0, f64::NAN, f64::INFINITY] {
            let level = SecurityLevel::custom_with_thresholds(
                "standard".to_string(),
                "standard".to_string(),
                0.4,
                bad,
                String::new(),
            );
            assert!(level.validate().is_err(), "{bad}");
        }

        for good in [MIN_SECURITY_THRESHOLD, 0.4, MAX_SECURITY_THRESHOLD] {
            let level = SecurityLevel::custom(
                "standard".to_string(),
                "standard".to_string(),
                good,
                String::new(),
            );
            level.validate().unwrap();
        }
    }

    #[test]
    fn the_highest_allowed_threshold_can_still_be_matched() {
        let perfect_score = 1.0_f32;
        assert!(
            perfect_score > MAX_SECURITY_THRESHOLD as f32,
            "a threshold of {MAX_SECURITY_THRESHOLD} can never be matched, because scores are dot products of unit vectors and matching requires score > threshold"
        );
    }

    #[test]
    fn an_existing_config_below_the_new_frame_minimum_falls_back_to_the_default() {
        for stale in 1..MIN_LIVENESS_MAX_FRAMES {
            let liveness = LivenessConfig {
                max_frames: stale,
                ..LivenessConfig::default()
            };
            assert!(liveness.validate().is_err(), "{stale} must be rejected");
            assert_eq!(
                liveness.effective_max_frames(),
                LivenessConfig::default().max_frames,
                "{stale} must fall back to the default rather than gate auth"
            );
        }
    }

    #[test]
    fn the_smallest_allowed_frame_budget_can_reach_sustained_liveness() {
        const {
            assert!(
                MIN_LIVENESS_MAX_FRAMES > 5,
                "liveness needs 5 scored frames before its sustained path triggers"
            )
        };
    }

    #[test]
    fn out_of_range_custom_threshold_never_reaches_the_matcher() {
        for bad in [-1.0, 0.0, 5.0, f64::NAN, f64::INFINITY] {
            let level = SecurityLevel::custom(
                "standard".to_string(),
                "standard".to_string(),
                bad,
                String::new(),
            );
            assert_eq!(
                level.threshold(),
                DEFAULT_SECURITY_THRESHOLD as f32,
                "{bad}"
            );
        }

        let level = SecurityLevel::custom(
            "standard".to_string(),
            "standard".to_string(),
            0.73,
            String::new(),
        );
        assert!((level.threshold() - 0.73).abs() < f32::EPSILON);
    }

    #[test]
    fn custom_security_thresholds_are_spectrum_specific() {
        let level = SecurityLevel::custom_with_thresholds(
            "standard".to_string(),
            "standard".to_string(),
            0.73,
            0.31,
            String::new(),
        );

        assert!((level.rgb_threshold() - 0.73).abs() < f32::EPSILON);
        assert!((level.ir_threshold() - 0.31).abs() < f32::EPSILON);
    }

    #[test]
    fn legacy_security_threshold_applies_to_both_spectra() {
        let config: Config = toml::from_str(
            r#"
            [security]
            level = "custom"
            detector = "standard"
            recognizer = "standard"
            threshold = 0.47
            "#,
        )
        .unwrap();

        assert!((config.security.rgb_threshold() - 0.47).abs() < f32::EPSILON);
        assert!((config.security.ir_threshold() - 0.47).abs() < f32::EPSILON);

        let migrated = toml::to_string(&config).unwrap();
        assert!(migrated.contains("rgb_threshold = 0.47"));
        assert!(migrated.contains("ir_threshold = 0.47"));

        let security_section = migrated
            .split("[security]")
            .nth(1)
            .and_then(|rest| rest.split("\n[").next())
            .unwrap_or_default();
        assert!(
            !security_section
                .lines()
                .any(|line| line.trim_start().starts_with("threshold =")),
            "the legacy security.threshold key must not be re-serialized: {security_section}"
        );
    }

    #[test]
    fn setting_one_spectrum_threshold_does_not_leak_into_the_other() {
        let config: Config = toml::from_str(
            r#"
            [security]
            level = "custom"
            detector = "standard"
            recognizer = "standard"
            rgb_threshold = 0.6
            "#,
        )
        .unwrap();

        assert!((config.security.rgb_threshold() - 0.6).abs() < f32::EPSILON);
        assert_eq!(
            config.security.ir_threshold(),
            DEFAULT_SECURITY_THRESHOLD as f32,
            "an unset ir_threshold must not silently inherit rgb_threshold"
        );
    }

    #[test]
    fn validate_rejects_unknown_hybrid_policy() {
        let bad = SecurityLevel::custom(
            "standard".to_string(),
            "standard".to_string(),
            0.5,
            "not-a-real-policy".to_string(),
        );
        assert!(bad.validate().is_err());

        for policy in HYBRID_POLICY_OPTIONS {
            let level = SecurityLevel::custom(
                "standard".to_string(),
                "standard".to_string(),
                0.5,
                policy.to_string(),
            );
            level.validate().unwrap();
        }

        let empty = SecurityLevel::custom(
            "standard".to_string(),
            "standard".to_string(),
            0.5,
            String::new(),
        );
        empty.validate().unwrap();
        assert_eq!(empty.hybrid_policy(), "fallback_on_dark");
    }

    #[test]
    fn validate_rejects_unknown_parallel_capture_mode() {
        let bad = CameraConfig {
            parallel_capture: "sometimes".to_string(),
            ..CameraConfig::default()
        };
        assert!(bad.validate().is_err());
        assert_eq!(bad.parallel_capture(), "never");

        for mode in PARALLEL_CAPTURE_OPTIONS {
            let cameras = CameraConfig {
                parallel_capture: mode.to_string(),
                ..CameraConfig::default()
            };
            cameras.validate().unwrap();
            assert_eq!(cameras.parallel_capture(), mode);
        }
    }

    #[test]
    fn parallel_capture_indices_round_trip() {
        for (index, mode) in PARALLEL_CAPTURE_OPTIONS.iter().enumerate() {
            let cameras = CameraConfig {
                parallel_capture: (*mode).to_string(),
                ..CameraConfig::default()
            };
            assert_eq!(cameras.parallel_capture_index(), index as u32);
            assert_eq!(
                CameraConfig::parallel_capture_from_index(index),
                (*mode).to_string()
            );
        }
        assert_eq!(
            CameraConfig::parallel_capture_from_index(PARALLEL_CAPTURE_OPTIONS.len()),
            "never".to_string()
        );
    }

    #[test]
    fn preset_levels_ignore_a_stored_custom_threshold() {
        for preset in ["low", "medium", "high", "maximum"] {
            let mut level = SecurityLevel::medium();
            level.level = preset.to_string();
            level.rgb_threshold = -1.0;
            level.ir_threshold = -1.0;
            level.validate().unwrap();
            assert!(level.threshold() > 0.0, "{preset}");
            assert!(level.ir_threshold() > 0.0, "{preset}");
        }
    }

    #[test]
    fn liveness_validate_rejects_degenerate_settings() {
        let mut cfg = LivenessConfig::default();
        cfg.validate().unwrap();

        for bad in [-1.0, 0.0, 0.09, 1.01, f64::NAN, f64::INFINITY] {
            cfg.threshold = bad;
            assert!(cfg.validate().is_err(), "{bad}");
            assert_eq!(cfg.effective_threshold(), default_liveness_threshold());
        }

        cfg.threshold = 0.9;
        assert_eq!(cfg.effective_threshold(), 0.9);

        cfg.max_frames = 0;
        assert!(cfg.validate().is_err());
        assert_eq!(cfg.effective_max_frames(), default_max_frames());

        cfg.max_frames = 25;
        cfg.validate().unwrap();
        assert_eq!(cfg.effective_max_frames(), 25);
    }

    #[test]
    fn load_from_falls_back_on_a_degenerate_custom_threshold() {
        let temp = TempDir::new("bad-threshold");
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "[security]\nlevel = \"custom\"\nthreshold = 0.0\n\n[liveness]\nthreshold = 0.0\nmax_frames = 0\n",
        )
        .unwrap();

        let config = Config::load_from(path.to_str().unwrap()).unwrap();
        assert_eq!(
            config.security.threshold(),
            DEFAULT_SECURITY_THRESHOLD as f32
        );
        assert_eq!(
            config.security.ir_threshold(),
            DEFAULT_SECURITY_THRESHOLD as f32
        );
        assert_eq!(
            config.liveness.effective_threshold(),
            default_liveness_threshold()
        );
        assert_eq!(config.liveness.effective_max_frames(), default_max_frames());
    }

    #[test]
    fn validate_rejects_unknown_security_level() {
        let mut level = SecurityLevel::medium();
        level.level = "bogus".to_string();
        assert!(level.validate().is_err());
        // Known presets still validate.
        for preset in ["low", "medium", "high", "maximum"] {
            let mut l = SecurityLevel::medium();
            l.level = preset.to_string();
            l.validate().unwrap();
        }
    }

    #[test]
    fn inference_config_accepts_supported_provider_device_pairs() {
        #[cfg(feature = "openvino-config")]
        let supported = [
            ("cpu", "cpu"),
            ("openvino", "cpu"),
            ("openvino", "gpu"),
            ("openvino", "npu"),
        ];
        #[cfg(not(feature = "openvino-config"))]
        let supported = [("cpu", "cpu")];

        for (execution_provider, device) in supported {
            let inference = InferenceConfig {
                execution_provider: execution_provider.to_string(),
                device: device.to_string(),
            };
            inference.validate().unwrap();
        }
    }

    #[test]
    fn inference_config_rejects_invalid_pairs() {
        #[cfg(feature = "openvino-config")]
        let invalid = [
            ("cpu", "gpu"),
            ("cpu", "npu"),
            ("openvino", "cuda"),
            ("webgpu", "gpu"),
        ];
        #[cfg(not(feature = "openvino-config"))]
        let invalid = [
            ("cpu", "gpu"),
            ("cpu", "npu"),
            ("openvino", "cuda"),
            ("webgpu", "gpu"),
            ("openvino", "cpu"),
            ("openvino", "gpu"),
            ("openvino", "npu"),
        ];

        for (execution_provider, device) in invalid {
            let inference = InferenceConfig {
                execution_provider: execution_provider.to_string(),
                device: device.to_string(),
            };
            assert!(inference.validate().is_err());
        }
    }

    #[test]
    fn a_value_this_build_cannot_show_is_not_representable() {
        let unknown = InferenceConfig {
            execution_provider: "webgpu".to_string(),
            device: "cuda".to_string(),
        };
        assert!(!unknown.is_representable());
        assert!(InferenceConfig::default().is_representable());

        let openvino = InferenceConfig {
            execution_provider: "openvino".to_string(),
            device: "npu".to_string(),
        };
        assert_eq!(
            openvino.is_representable(),
            cfg!(feature = "openvino-config")
        );
    }

    #[test]
    fn unknown_level_falls_back_to_medium_without_panicking() {
        let mut level = SecurityLevel::medium();
        level.level = "bogus".to_string();
        assert_eq!(level.detector(), "det_500m.onnx");
        assert_eq!(level.recognizer(), "w600k_mbf.onnx");
        assert!((level.threshold() - 0.4).abs() < f32::EPSILON);
    }

    #[test]
    fn load_from_tolerates_invalid_level_with_fallback() {
        let temp = TempDir::new("bad-level");
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[security]\nlevel = \"bogus\"\n").unwrap();

        let config = Config::load_from(path.to_str().unwrap()).unwrap();
        assert_eq!(config.security.detector(), "det_500m.onnx");
    }

    #[test]
    fn misspelled_keys_are_reported_as_unknown() {
        let contents =
            "[storage]\nencrypt_template = true\n\n[auth]\nrequire_confirmation_elavation = true\n";
        let unknown = unknown_config_keys(contents);
        assert_eq!(
            unknown,
            vec![
                "auth.require_confirmation_elavation".to_string(),
                "storage.encrypt_template".to_string(),
            ]
        );
    }

    #[test]
    fn correctly_spelled_and_legacy_keys_are_not_reported() {
        let contents = "[storage]\nencrypt_templates = true\n\n[auth]\nrequire_confirmation = true\nrequire_confirmation_elevation = true\n\n[security]\nthreshold = 0.5\n";
        assert!(unknown_config_keys(contents).is_empty());
    }

    #[test]
    fn an_unknown_table_is_reported() {
        assert_eq!(
            unknown_config_keys("[storrage]\nencrypt_templates = true\n"),
            vec!["storrage".to_string()]
        );
    }

    #[test]
    fn the_packaged_config_has_no_unknown_keys() {
        let contents = include_str!("../../packaging/config/config.toml");
        assert!(unknown_config_keys(contents).is_empty());
    }

    #[test]
    fn the_dbus_signature_is_unchanged() {
        use zvariant::Type;
        assert_eq!(AuthConfig::SIGNATURE.to_string(), "(bbbbtts)");
        assert_eq!(
            Config::SIGNATURE.to_string(),
            "((ss)(sssdds)(ssby)(bbbbtts)(ud)(bdu)(b))"
        );
    }

    #[test]
    fn legacy_require_confirmation_applies_to_both_surfaces() {
        let auth: AuthConfig = toml::from_str("require_confirmation = true\n").unwrap();
        assert!(auth.require_confirmation_lock_screen);
        assert!(auth.require_confirmation_elevation);
        assert!(auth.requires_confirmation(AuthSurface::Elevation));
        assert!(auth.requires_confirmation(AuthSurface::ScreenLock));
    }

    #[test]
    fn an_explicit_split_key_overrides_the_legacy_key() {
        let auth: AuthConfig =
            toml::from_str("require_confirmation = true\nrequire_confirmation_elevation = false\n")
                .unwrap();
        assert!(auth.require_confirmation_lock_screen);
        assert!(!auth.require_confirmation_elevation);
    }

    #[test]
    fn confirmation_defaults_to_off_without_any_key() {
        let auth: AuthConfig = toml::from_str("").unwrap();
        assert!(!auth.require_confirmation_lock_screen);
        assert!(!auth.require_confirmation_elevation);
    }

    #[test]
    fn load_from_keeps_loading_when_a_key_is_misspelled() {
        let temp = TempDir::new("typo-key");
        let path = temp.path().join("config.toml");
        std::fs::write(
            &path,
            "[storage]\nencrypt_template = true\n\n[security]\nlevel = \"high\"\n",
        )
        .unwrap();

        let config = Config::load_from(path.to_str().unwrap()).unwrap();
        assert!(!config.storage.encrypt_templates);
        assert_eq!(config.security.level, "high");
        assert_eq!(
            unknown_config_keys(&std::fs::read_to_string(&path).unwrap()),
            vec!["storage.encrypt_template".to_string()]
        );
    }

    #[test]
    fn load_from_honours_the_legacy_confirmation_key() {
        let temp = TempDir::new("legacy-confirmation");
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "[auth]\nrequire_confirmation = true\n").unwrap();

        let config = Config::load_from(path.to_str().unwrap()).unwrap();
        assert!(config.auth.require_confirmation_lock_screen);
        assert!(config.auth.require_confirmation_elevation);
        assert!(unknown_config_keys(&std::fs::read_to_string(&path).unwrap()).is_empty());
    }

    #[test]
    fn save_to_keeps_comments_and_file_mode() {
        let temp = TempDir::new("preserve-comments");
        let path = temp.path().join("config.toml");
        let original = include_str!("../../packaging/config/config.toml");
        fs::write(&path, original).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let comments_before = original
            .lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count();
        assert!(comments_before > 20, "fixture should be comment-heavy");

        let mut config = Config::load_from(path.to_str().unwrap()).unwrap();
        config.security.level = "high".to_string();
        config.save_to(path.to_str().unwrap()).unwrap();

        let rewritten = fs::read_to_string(&path).unwrap();
        let comments_after = rewritten
            .lines()
            .filter(|l| l.trim_start().starts_with('#'))
            .count();
        assert_eq!(
            comments_after, comments_before,
            "every comment must survive a save"
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644,
            "an existing file's mode must not change"
        );
        assert_eq!(
            Config::load_from(path.to_str().unwrap())
                .unwrap()
                .security
                .level,
            "high",
            "the edited value must actually be written"
        );
    }

    #[test]
    fn save_to_a_new_file_uses_the_packaged_mode() {
        let temp = TempDir::new("new-config-mode");
        let path = temp.path().join("config.toml");
        Config::default().save_to(path.to_str().unwrap()).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
    }

    #[test]
    fn save_to_preserves_a_tightened_mode() {
        let temp = TempDir::new("tight-config-mode");
        let path = temp.path().join("config.toml");
        fs::write(&path, "[security]\nlevel = \"medium\"\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        Config::default().save_to(path.to_str().unwrap()).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "a deliberately tightened mode must be kept"
        );
    }

    #[test]
    fn load_from_missing_file_returns_default() {
        let temp = TempDir::new("missing");
        let path = temp.path().join("missing.toml");

        let config = Config::load_from(path.to_str().unwrap()).unwrap();
        assert_eq!(
            config.security.detector(),
            SecurityLevel::medium().detector()
        );
        assert_eq!(config.inference.execution_provider, "cpu");
        assert_eq!(config.inference.device, "cpu");
        assert_eq!(config.cameras.rgb, DEFAULT_RGB_CAMERA);
        assert_eq!(config.cameras.dark_luma_threshold, 20);
        assert_eq!(config.cameras.parallel_capture(), "never");
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert_eq!(config.enrollment.max_templates, 2);
        assert_eq!(
            config.enrollment.min_face_size_ratio,
            DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO
        );
        assert!(!config.storage.encrypt_templates);
    }

    #[test]
    fn save_to_and_load_from_round_trip() {
        let temp = TempDir::new("round-trip");
        let path = temp.path().join("config.toml");
        let config = Config {
            inference: InferenceConfig {
                execution_provider: "openvino".to_string(),
                device: "gpu".to_string(),
            },
            security: SecurityLevel::high(),
            cameras: CameraConfig {
                rgb: "primary".to_string(),
                ir: "pipewiresrc target-object=some-ir-camera".to_string(),
                emitter_enabled: true,
                dark_luma_threshold: 55,
                parallel_capture: "auto".to_string(),
            },
            auth: AuthConfig {
                abort_if_ssh: true,
                abort_if_lid_closed: false,
                require_confirmation_lock_screen: true,
                require_confirmation_elevation: true,
                resume_grace_ms: 3000,
                start_delay_ms: 1500,
                start_delay_scope: "screen_lock".to_string(),
            },
            enrollment: EnrollmentConfig {
                max_templates: 8,
                min_face_size_ratio: 0.20,
            },
            liveness: LivenessConfig {
                enabled: true,
                threshold: 0.9,
                max_frames: 25,
            },
            storage: StorageConfig {
                encrypt_templates: true,
            },
        };

        config.save_to(path.to_str().unwrap()).unwrap();
        let loaded = Config::load_from(path.to_str().unwrap()).unwrap();

        assert_eq!(loaded.security.detector(), SecurityLevel::high().detector());
        assert_eq!(loaded.inference.execution_provider, "openvino");
        assert_eq!(loaded.inference.device, "gpu");
        assert_eq!(
            loaded.security.recognizer(),
            SecurityLevel::high().recognizer()
        );
        assert_eq!(loaded.cameras.rgb, "primary");
        assert_eq!(
            loaded.cameras.ir,
            "pipewiresrc target-object=some-ir-camera"
        );
        assert!(loaded.cameras.emitter_enabled);
        assert_eq!(loaded.cameras.dark_luma_threshold, 55);
        assert_eq!(loaded.cameras.parallel_capture(), "auto");
        assert!(loaded.auth.abort_if_ssh);
        assert!(!loaded.auth.abort_if_lid_closed);
        assert!(loaded.auth.require_confirmation_lock_screen);
        assert!(loaded.auth.require_confirmation_elevation);
        assert_eq!(loaded.auth.resume_grace_ms, 3000);
        assert_eq!(loaded.auth.start_delay_ms, 1500);
        assert_eq!(loaded.auth.start_delay_scope(), "screen_lock");
        assert_eq!(loaded.enrollment.max_templates, 8);
        assert_eq!(loaded.enrollment.min_face_size_ratio, 0.20);
        assert!(loaded.liveness.enabled);
        assert_eq!(loaded.liveness.threshold, 0.9);
        assert_eq!(loaded.liveness.max_frames, 25);
        assert!(loaded.storage.encrypt_templates);
    }

    #[test]
    fn ir_camera_fields_default_empty_and_disabled() {
        let config: Config = toml::from_str(
            r#"
            [cameras]
            rgb = "primary"
            "#,
        )
        .unwrap();

        assert_eq!(config.cameras.ir, "");
        assert!(!config.cameras.emitter_enabled);
    }

    #[test]
    fn partial_toml_uses_liveness_serde_defaults() {
        let config: Config = toml::from_str(
            r#"
            [liveness]
            enabled = true
            "#,
        )
        .unwrap();

        assert!(config.liveness.enabled);
        assert!((config.liveness.threshold - 0.8).abs() < f64::EPSILON);
        assert_eq!(config.liveness.max_frames, 40);
    }

    #[test]
    fn partial_toml_uses_serde_defaults() {
        let config: Config = toml::from_str(
            r#"
            [security]
            level = "maximum"
            "#,
        )
        .unwrap();

        assert_eq!(
            config.security.detector(),
            SecurityLevel::maximum().detector()
        );
        assert_eq!(config.cameras.rgb, DEFAULT_RGB_CAMERA);
        assert_eq!(config.cameras.dark_luma_threshold, 20);
        assert_eq!(config.cameras.parallel_capture(), "never");
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert!(!config.auth.require_confirmation_lock_screen);
        assert!(!config.auth.require_confirmation_elevation);
        assert_eq!(config.auth.start_delay_ms, 0);
        assert_eq!(config.enrollment.max_templates, 2);
        assert_eq!(
            config.enrollment.min_face_size_ratio,
            DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO
        );
        assert!(!config.storage.encrypt_templates);
    }

    #[test]
    fn config_round_trips_through_the_dbus_value_used_by_the_config_property() {
        let mut cfg = Config::default();
        cfg.auth.start_delay_ms = 4500;
        cfg.auth.resume_grace_ms = 1500;
        cfg.security = SecurityLevel::custom_with_thresholds(
            "standard".to_string(),
            "standard".to_string(),
            0.61,
            0.29,
            String::new(),
        );

        let value = zvariant::OwnedValue::try_from(cfg).unwrap();
        let back = Config::try_from(value).unwrap();

        assert_eq!(back.auth.start_delay_ms, 4500);
        assert_eq!(back.auth.resume_grace_ms, 1500);
        assert!((back.security.rgb_threshold() - 0.61).abs() < f32::EPSILON);
        assert!((back.security.ir_threshold() - 0.29).abs() < f32::EPSILON);
    }

    #[test]
    fn start_delay_outside_a_lock_does_not_stack_with_resume_grace() {
        let mut auth = AuthConfig::default();
        let lock = AuthSurface::ScreenLock;

        assert_eq!(auth.effective_start_delay_ms(false, lock), 0);
        assert_eq!(auth.effective_start_delay_ms(true, lock), 0);

        auth.start_delay_ms = 5000;
        assert_eq!(auth.effective_start_delay_ms(false, lock), 5000);
        assert_eq!(auth.effective_start_delay_ms(true, lock), 5000);

        auth.resume_grace_ms = 3000;
        assert_eq!(auth.effective_start_delay_ms(false, lock), 5000);
        assert_eq!(auth.effective_start_delay_ms(true, lock), 5000);

        auth.resume_grace_ms = 8000;
        assert_eq!(auth.effective_start_delay_ms(false, lock), 5000);
        assert_eq!(auth.effective_start_delay_ms(true, lock), 8000);

        auth.start_delay_ms = 0;
        assert_eq!(auth.effective_start_delay_ms(false, lock), 0);
        assert_eq!(auth.effective_start_delay_ms(true, lock), 8000);
    }

    #[test]
    fn default_scope_spares_elevation_and_login() {
        let auth = AuthConfig {
            start_delay_ms: 3000,
            ..Default::default()
        };

        assert_eq!(auth.start_delay_scope(), "screen_lock");
        assert_eq!(
            auth.effective_start_delay_ms(false, AuthSurface::ScreenLock),
            3000
        );
        for surface in [AuthSurface::Elevation, AuthSurface::Login] {
            assert_eq!(auth.effective_start_delay_ms(false, surface), 0);
        }
    }

    #[test]
    fn start_delay_is_measured_from_the_lock_not_each_auth() {
        let auth = AuthConfig {
            start_delay_ms: 3000,
            ..Default::default()
        };
        let lock = AuthSurface::ScreenLock;

        assert_eq!(auth.start_delay_after_lock_ms(false, lock, None), 3000);
        assert_eq!(auth.start_delay_after_lock_ms(false, lock, Some(0)), 3000);
        assert_eq!(
            auth.start_delay_after_lock_ms(false, lock, Some(1000)),
            2000
        );
        assert_eq!(auth.start_delay_after_lock_ms(false, lock, Some(3000)), 0);
        assert_eq!(auth.start_delay_after_lock_ms(false, lock, Some(60_000)), 0);
    }

    #[test]
    fn resume_grace_survives_an_already_elapsed_lock_window() {
        let auth = AuthConfig {
            start_delay_ms: 3000,
            resume_grace_ms: 2000,
            ..Default::default()
        };
        let lock = AuthSurface::ScreenLock;

        assert_eq!(auth.start_delay_after_lock_ms(false, lock, Some(60_000)), 0);
        assert_eq!(
            auth.start_delay_after_lock_ms(true, lock, Some(60_000)),
            2000
        );
    }

    #[test]
    fn elapsed_lock_time_never_revives_a_scoped_out_surface() {
        let auth = AuthConfig {
            start_delay_ms: 3000,
            ..Default::default()
        };

        for surface in [AuthSurface::Elevation, AuthSurface::Login] {
            assert_eq!(auth.start_delay_after_lock_ms(false, surface, None), 0);
            assert_eq!(auth.start_delay_after_lock_ms(false, surface, Some(0)), 0);
        }
    }

    #[test]
    fn screen_lock_scope_exempts_elevation_and_login() {
        let auth = AuthConfig {
            start_delay_ms: 3000,
            start_delay_scope: "screen_lock".to_string(),
            ..Default::default()
        };

        assert_eq!(
            auth.effective_start_delay_ms(false, AuthSurface::ScreenLock),
            3000
        );
        assert_eq!(
            auth.effective_start_delay_ms(false, AuthSurface::Elevation),
            0
        );
        assert_eq!(auth.effective_start_delay_ms(false, AuthSurface::Login), 0);
    }

    #[test]
    fn resume_grace_is_not_scoped_away_by_screen_lock() {
        let auth = AuthConfig {
            start_delay_ms: 3000,
            resume_grace_ms: 5000,
            start_delay_scope: "screen_lock".to_string(),
            ..Default::default()
        };

        assert_eq!(
            auth.effective_start_delay_ms(true, AuthSurface::Elevation),
            5000
        );
        assert_eq!(
            auth.effective_start_delay_ms(false, AuthSurface::Elevation),
            0
        );
    }

    #[test]
    fn invalid_scope_falls_back_to_delaying_everything() {
        let auth = AuthConfig {
            start_delay_ms: 3000,
            start_delay_scope: "lockscreen".to_string(),
            ..Default::default()
        };

        assert_eq!(auth.start_delay_scope(), "all");
        assert_eq!(
            auth.effective_start_delay_ms(false, AuthSurface::Elevation),
            3000
        );
    }

    #[test]
    fn lock_screen_and_elevation_confirmation_toggles_are_independent() {
        let lock_only = AuthConfig {
            require_confirmation_lock_screen: true,
            require_confirmation_elevation: false,
            ..Default::default()
        };
        assert!(lock_only.requires_confirmation(AuthSurface::ScreenLock));
        assert!(lock_only.requires_confirmation(AuthSurface::Login));
        assert!(!lock_only.requires_confirmation(AuthSurface::Elevation));
        assert!(!lock_only.requires_confirmation(AuthSurface::Direct));

        let elevation_only = AuthConfig {
            require_confirmation_lock_screen: false,
            require_confirmation_elevation: true,
            ..Default::default()
        };
        assert!(!elevation_only.requires_confirmation(AuthSurface::ScreenLock));
        assert!(!elevation_only.requires_confirmation(AuthSurface::Login));
        assert!(elevation_only.requires_confirmation(AuthSurface::Elevation));
        assert!(!elevation_only.requires_confirmation(AuthSurface::Direct));
    }

    #[test]
    fn elevation_and_login_services_are_classified() {
        for service in ["sudo", "sudo-i", "su", "su-l", "doas", "polkit-1", "pkexec"] {
            assert_eq!(
                classify_pam_service(Some(service)),
                AuthSurface::Elevation,
                "{service}"
            );
        }
        for service in ["login", "sddm", "lightdm", "greetd", "gdm-password"] {
            assert_eq!(
                classify_pam_service(Some(service)),
                AuthSurface::Login,
                "{service}"
            );
        }
    }

    #[test]
    fn unknown_and_ambiguous_services_are_treated_as_screen_locks() {
        for service in [
            "hyprlock-gaze",
            "hyprlock-gaze-simultaneous",
            "gaze",
            "swaylock",
            "some-locker-nobody-has-heard-of",
            "gdm-face",
        ] {
            assert_eq!(
                classify_pam_service(Some(service)),
                AuthSurface::ScreenLock,
                "{service}"
            );
        }
    }

    #[test]
    fn callers_with_no_pam_service_are_direct() {
        assert_eq!(classify_pam_service(None), AuthSurface::Direct);
        assert_eq!(classify_pam_service(Some("")), AuthSurface::Direct);
    }

    #[test]
    fn direct_callers_never_wait_out_the_start_delay() {
        for scope in ["screen_lock", "all"] {
            let auth = AuthConfig {
                start_delay_ms: 3000,
                resume_grace_ms: 5000,
                start_delay_scope: scope.to_string(),
                ..Default::default()
            };

            assert_eq!(
                auth.effective_start_delay_ms(false, AuthSurface::Direct),
                0,
                "{scope}"
            );
            assert_eq!(
                auth.effective_start_delay_ms(true, AuthSurface::Direct),
                5000,
                "{scope}"
            );
            assert_eq!(
                auth.effective_start_delay_ms(false, AuthSurface::ScreenLock),
                3000,
                "{scope}"
            );
        }
    }

    #[test]
    fn enrollment_face_size_ratio_validates_and_falls_back_safely() {
        let mut enrollment = EnrollmentConfig::default();
        assert!(enrollment.validate().is_ok());
        assert_eq!(
            enrollment.effective_min_face_size_ratio(),
            DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO as f32
        );

        enrollment.min_face_size_ratio = 0.20;
        assert!(enrollment.validate().is_ok());
        assert_eq!(enrollment.effective_min_face_size_ratio(), 0.20);

        for invalid in [0.0, 0.09, 0.76, f64::NAN, f64::INFINITY] {
            enrollment.min_face_size_ratio = invalid;
            assert!(enrollment.validate().is_err());
            assert_eq!(
                enrollment.effective_min_face_size_ratio(),
                DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO as f32
            );
        }
    }

    #[test]
    fn storage_encrypt_templates_parses_and_defaults_false() {
        let enabled: Config = toml::from_str(
            r#"
            [storage]
            encrypt_templates = true
            "#,
        )
        .unwrap();
        assert!(enabled.storage.encrypt_templates);

        // Selecting a security preset must not disturb the storage table.
        let mut cfg = enabled.clone();
        cfg.security = SecurityLevel::high();
        assert!(cfg.storage.encrypt_templates);

        let absent: Config = toml::from_str(
            r#"[security]
level = "low""#,
        )
        .unwrap();
        assert!(!absent.storage.encrypt_templates);
    }

    #[test]
    fn hybrid_policy_mappings() {
        let mut config = Config::default();

        config.security.level = "low".to_string();
        assert_eq!(config.security.hybrid_policy(), "or");

        config.security.level = "medium".to_string();
        assert_eq!(config.security.hybrid_policy(), "fallback_on_dark");

        config.security.level = "high".to_string();
        assert_eq!(config.security.hybrid_policy(), "fallback_on_dark");

        config.security.level = "maximum".to_string();
        assert_eq!(config.security.hybrid_policy(), "and");

        config.security.level = "unknown".to_string();
        assert_eq!(config.security.hybrid_policy(), "fallback_on_dark");

        config.security.level = "custom".to_string();
        config.security.hybrid_policy = "custom_policy".to_string();
        assert_eq!(config.security.hybrid_policy(), "custom_policy");
    }

    #[test]
    fn validation_errors_accumulate_across_fields() {
        let level = SecurityLevel::custom_with_thresholds(
            "sharp".to_string(),
            "standard".to_string(),
            1.5,
            -0.1,
            "sometimes".to_string(),
        );

        let fields: Vec<SecurityField> = level
            .validation_errors()
            .iter()
            .map(|err| err.field)
            .collect();
        assert_eq!(
            fields,
            vec![
                SecurityField::ModelQuality,
                SecurityField::Threshold,
                SecurityField::Threshold,
                SecurityField::HybridPolicy,
            ]
        );
    }

    #[test]
    fn option_lists_and_labels_stay_aligned() {
        assert_eq!(SECURITY_LEVEL_OPTIONS.len(), SECURITY_LEVEL_LABELS.len());
        assert_eq!(MODEL_QUALITY_OPTIONS.len(), MODEL_QUALITY_LABELS.len());
        assert_eq!(HYBRID_POLICY_OPTIONS.len(), HYBRID_POLICY_LABELS.len());
        assert_eq!(
            START_DELAY_SCOPE_OPTIONS.len(),
            START_DELAY_SCOPE_LABELS.len()
        );
    }

    #[test]
    fn validate_reports_only_the_first_error() {
        let level = SecurityLevel::custom_with_thresholds(
            "standard".to_string(),
            "standard".to_string(),
            1.5,
            -0.1,
            String::new(),
        );

        let message = level.validate().unwrap_err().to_string();
        assert!(message.contains("security.rgb_threshold"), "{message}");
        assert_eq!(level.validation_errors().len(), 2);
    }

    #[test]
    fn an_unknown_level_stops_before_the_custom_checks() {
        let mut level = SecurityLevel::custom_with_thresholds(
            "sharp".to_string(),
            "sharp".to_string(),
            1.5,
            -0.1,
            "sometimes".to_string(),
        );
        level.level = "paranoid".to_string();

        let errors = level.validation_errors();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, SecurityField::Level);
    }

    #[test]
    fn custom_level_index_tracks_the_option_list() {
        assert_eq!(
            SecurityLevel::CUSTOM_LEVEL_INDEX as usize,
            SECURITY_LEVEL_OPTIONS
                .iter()
                .position(|level| *level == "custom")
                .expect("custom must be listed")
        );
    }

    #[test]
    fn index_helpers_fall_back_when_unlisted() {
        assert_eq!(index_of(&SECURITY_LEVEL_OPTIONS, "high", 1), 2);
        assert_eq!(index_of(&SECURITY_LEVEL_OPTIONS, "nonsense", 1), 1);
        assert_eq!(value_at(&MODEL_QUALITY_OPTIONS, 1, "standard"), "accurate");
        assert_eq!(value_at(&MODEL_QUALITY_OPTIONS, 9, "standard"), "standard");
    }

    #[test]
    fn custom_form_seeds_from_the_resolved_preset() {
        let seed = SecurityLevel::high().custom_form();
        assert_eq!(seed.detector, "accurate");
        assert_eq!(seed.recognizer, "accurate");
        assert_eq!(seed.rgb_threshold, 0.5);
        assert_eq!(seed.ir_threshold, 0.5);
        assert_eq!(seed.hybrid_policy, "fallback_on_dark");
    }

    #[test]
    fn custom_form_keeps_stored_custom_values() {
        let level = SecurityLevel::custom_with_thresholds(
            "standard".to_string(),
            "accurate".to_string(),
            0.31,
            0.62,
            "and".to_string(),
        );
        let seed = level.custom_form();
        assert_eq!(seed.detector, "standard");
        assert_eq!(seed.recognizer, "accurate");
        assert_eq!(seed.rgb_threshold, 0.31);
        assert_eq!(seed.ir_threshold, 0.62);
        assert_eq!(seed.hybrid_policy, "and");
    }
}
