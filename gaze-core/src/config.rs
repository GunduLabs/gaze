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
    30
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
}

fn default_false() -> bool {
    false
}

fn default_resume_grace_ms() -> u64 {
    0
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Serialize, Clone, Debug, Value, OwnedValue, Type)]
pub struct EnrollmentConfig {
    #[serde(default = "default_max_templates")]
    pub max_templates: u32,
}

fn default_max_templates() -> u32 {
    2
}

impl Default for EnrollmentConfig {
    fn default() -> Self {
        Self {
            max_templates: default_max_templates(),
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
        assert_eq!(config.cameras.rgb, DEFAULT_RGB_CAMERA);
        assert_eq!(config.cameras.dark_luma_threshold, 30);
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert_eq!(config.enrollment.max_templates, 2);
        assert!(!config.storage.encrypt_templates);
    }

    #[test]
    fn save_to_and_load_from_round_trip() {
        let temp = TempDir::new("round-trip");
        let path = temp.path().join("config.toml");
        let config = Config {
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
            },
            enrollment: EnrollmentConfig { max_templates: 8 },
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
        assert_eq!(loaded.enrollment.max_templates, 8);
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
        assert_eq!(config.cameras.dark_luma_threshold, 30);
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert!(!config.auth.require_confirmation);
        assert_eq!(config.enrollment.max_templates, 2);
        assert!(!config.storage.encrypt_templates);
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
