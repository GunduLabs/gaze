use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use zvariant::{OwnedValue, Type, Value};

pub const CONFIG_PATH: &str = "/etc/gaze/config.toml";
pub const USERS_DIR: &str = "/var/lib/gaze/users";
pub const MODELS_DIR: &str = "/var/cache/gaze";
pub const DEFAULT_RGB_CAMERA: &str = "primary";
pub const SECURITY_LEVEL_OPTIONS: [&str; 5] = ["low", "medium", "high", "maximum", "custom"];
pub const MODEL_QUALITY_OPTIONS: [&str; 2] = ["standard", "accurate"];
pub const HYBRID_POLICY_OPTIONS: [&str; 4] = ["default", "or", "fallback_on_dark", "and"];
pub const START_DELAY_SCOPE_OPTIONS: [&str; 2] = ["all", "screen_lock"];
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

fn default_level() -> String {
    "medium".to_string()
}

#[derive(Deserialize, Serialize, Clone, Debug, Value, OwnedValue, Type)]
pub struct SecurityLevel {
    #[serde(default = "default_level")]
    pub level: String,
    #[serde(default)]
    pub detector: String,
    #[serde(default)]
    pub recognizer: String,
    #[serde(default)]
    pub threshold: f64,
    #[serde(default)]
    pub hybrid_policy: String,
}

impl Default for SecurityLevel {
    fn default() -> Self {
        Self {
            level: default_level(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
            hybrid_policy: String::new(),
        }
    }
}

impl SecurityLevel {
    pub const CUSTOM_LEVEL_INDEX: u32 = 4;

    pub fn low() -> Self {
        Self {
            level: "low".to_string(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
            hybrid_policy: String::new(),
        }
    }

    pub fn medium() -> Self {
        Self {
            level: "medium".to_string(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
            hybrid_policy: String::new(),
        }
    }

    pub fn high() -> Self {
        Self {
            level: "high".to_string(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
            hybrid_policy: String::new(),
        }
    }

    pub fn maximum() -> Self {
        Self {
            level: "maximum".to_string(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
            hybrid_policy: String::new(),
        }
    }

    pub fn custom(
        detector: String,
        recognizer: String,
        threshold: f64,
        hybrid_policy: String,
    ) -> Self {
        Self {
            level: "custom".to_string(),
            detector,
            recognizer,
            threshold,
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
        SECURITY_LEVEL_OPTIONS
            .iter()
            .position(|level| *level == self.level.as_str())
            .map(|idx| idx as u32)
            .unwrap_or(1)
    }

    pub fn model_quality_index(value: &str) -> u32 {
        MODEL_QUALITY_OPTIONS
            .iter()
            .position(|quality| *quality == value)
            .map(|idx| idx as u32)
            .unwrap_or(0)
    }

    pub fn model_quality_from_index(index: usize) -> &'static str {
        MODEL_QUALITY_OPTIONS
            .get(index)
            .copied()
            .unwrap_or("standard")
    }

    pub fn hybrid_policy_index_for_value(value: &str) -> u32 {
        HYBRID_POLICY_OPTIONS
            .iter()
            .position(|policy| *policy == value)
            .map(|idx| idx as u32)
            .unwrap_or(0)
    }

    pub fn hybrid_policy_from_index(index: usize) -> String {
        if index == 0 {
            String::new()
        } else {
            HYBRID_POLICY_OPTIONS
                .get(index)
                .copied()
                .unwrap_or_default()
                .to_string()
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

    pub fn validate(&self) -> anyhow::Result<()> {
        if !SECURITY_LEVEL_OPTIONS.contains(&self.level.as_str()) {
            anyhow::bail!(
                "invalid security level {:?}: expected one of {:?}",
                self.level,
                SECURITY_LEVEL_OPTIONS
            );
        }
        if self.level == "custom" {
            match self.detector.as_str() {
                "standard" | "accurate" => {}
                other => anyhow::bail!(
                    "invalid detector level {other:?}: expected \"standard\" or \"accurate\""
                ),
            }
            match self.recognizer.as_str() {
                "standard" | "accurate" => {}
                other => anyhow::bail!(
                    "invalid recognizer level {other:?}: expected \"standard\" or \"accurate\""
                ),
            }
        }
        Ok(())
    }

    pub fn threshold(&self) -> f32 {
        match self.level.as_str() {
            "low" => 0.3,
            "medium" => 0.4,
            "high" => 0.5,
            "maximum" => 0.6,
            "custom" => self.threshold as f32,
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
        INFERENCE_EXECUTION_PROVIDER_OPTIONS
            .iter()
            .position(|value| *value == self.execution_provider)
            .map(|index| index as u32)
            .unwrap_or(0)
    }

    pub fn device_index(&self) -> u32 {
        INFERENCE_DEVICE_OPTIONS
            .iter()
            .position(|value| *value == self.device)
            .map(|index| index as u32)
            .unwrap_or(0)
    }

    pub fn execution_provider_from_index(index: usize) -> &'static str {
        INFERENCE_EXECUTION_PROVIDER_OPTIONS
            .get(index)
            .copied()
            .unwrap_or("cpu")
    }

    pub fn device_from_index(index: usize) -> &'static str {
        INFERENCE_DEVICE_OPTIONS
            .get(index)
            .copied()
            .unwrap_or("cpu")
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
}

fn default_rgb_device() -> String {
    DEFAULT_RGB_CAMERA.to_string()
}

fn default_dark_luma_threshold() -> u8 {
    20
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthSurface {
    ScreenLock,
    Elevation,
    Login,
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
        Some(name) if ELEVATION_SERVICES.contains(&name) => AuthSurface::Elevation,
        Some(name) if LOGIN_SERVICES.contains(&name) => AuthSurface::Login,
        _ => AuthSurface::ScreenLock,
    }
}

fn default_start_delay_scope() -> String {
    "all".to_string()
}

#[derive(Deserialize, Serialize, Clone, Debug, Value, OwnedValue, Type)]
pub struct AuthConfig {
    #[serde(default = "default_true")]
    pub abort_if_ssh: bool,
    #[serde(default = "default_true")]
    pub abort_if_lid_closed: bool,
    #[serde(default = "default_false")]
    pub require_confirmation: bool,
    #[serde(default = "default_resume_grace_ms")]
    pub resume_grace_ms: u64,
    #[serde(default = "default_start_delay_ms")]
    pub start_delay_ms: u64,
    #[serde(default = "default_start_delay_scope")]
    pub start_delay_scope: String,
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
        match self.start_delay_scope() {
            "screen_lock" => surface == AuthSurface::ScreenLock,
            _ => true,
        }
    }

    /// Milliseconds to wait before face verification begins.
    pub fn effective_start_delay_ms(&self, resumed: bool, surface: AuthSurface) -> u64 {
        let start = if self.start_delay_applies_to(surface) {
            self.start_delay_ms
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
        START_DELAY_SCOPE_OPTIONS
            .iter()
            .position(|scope| *scope == value)
            .map(|idx| idx as u32)
            .unwrap_or(0)
    }

    pub fn start_delay_scope_from_index(index: usize) -> String {
        START_DELAY_SCOPE_OPTIONS
            .get(index)
            .copied()
            .unwrap_or("all")
            .to_string()
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
            require_confirmation: false,
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
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(CONFIG_PATH)
    }

    pub fn load_from(path: &str) -> anyhow::Result<Self> {
        if Path::new(path).exists() {
            let contents = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&contents)?;
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
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(CONFIG_PATH)
    }

    pub fn save_to(&self, path: &str) -> anyhow::Result<()> {
        let encoded = toml::to_string_pretty(self).context("failed to serialize config")?;
        let path = Path::new(path);
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
        std::fs::rename(&tmp_path, path)
            .with_context(|| format!("failed to replace config file: {}", path.display()))?;
        Ok(())
    }
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
            },
            auth: AuthConfig {
                abort_if_ssh: true,
                abort_if_lid_closed: false,
                require_confirmation: true,
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
        assert!(loaded.auth.abort_if_ssh);
        assert!(!loaded.auth.abort_if_lid_closed);
        assert!(loaded.auth.require_confirmation);
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
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert!(!config.auth.require_confirmation);
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

        let value = zvariant::OwnedValue::try_from(cfg).unwrap();
        let back = Config::try_from(value).unwrap();

        assert_eq!(back.auth.start_delay_ms, 4500);
        assert_eq!(back.auth.resume_grace_ms, 1500);
    }

    #[test]
    fn start_delay_applies_on_every_auth_and_does_not_stack_with_resume_grace() {
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
    fn default_scope_delays_every_surface() {
        let auth = AuthConfig {
            start_delay_ms: 3000,
            ..Default::default()
        };

        assert_eq!(auth.start_delay_scope(), "all");
        for surface in [
            AuthSurface::ScreenLock,
            AuthSurface::Elevation,
            AuthSurface::Login,
        ] {
            assert_eq!(auth.effective_start_delay_ms(false, surface), 3000);
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

        assert_eq!(classify_pam_service(None), AuthSurface::ScreenLock);
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
}
