use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use zvariant::{OwnedValue, Type, Value};

const DEFAULT_CONFIG_PATH: &str = "/etc/gaze/config.toml";
pub const USERS_DIR: &str = "/var/lib/gaze/users";
pub const MODELS_DIR: &str = "/var/cache/gaze";
pub const DEFAULT_RGB_CAMERA: &str = "primary";

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
}

impl Default for SecurityLevel {
    fn default() -> Self {
        Self {
            level: default_level(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
        }
    }
}

impl SecurityLevel {
    pub fn low() -> Self {
        Self {
            level: "low".to_string(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
        }
    }

    pub fn medium() -> Self {
        Self {
            level: "medium".to_string(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
        }
    }

    pub fn high() -> Self {
        Self {
            level: "high".to_string(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
        }
    }

    pub fn maximum() -> Self {
        Self {
            level: "maximum".to_string(),
            detector: String::new(),
            recognizer: String::new(),
            threshold: 0.0,
        }
    }

    pub fn custom(detector: String, recognizer: String, threshold: f64) -> Self {
        Self {
            level: "custom".to_string(),
            detector,
            recognizer,
            threshold,
        }
    }

    pub fn detector(&self) -> &str {
        match self.level.as_str() {
            "low" | "medium" => "det_500m.onnx",
            "high" | "maximum" => "det_10g.onnx",
            "custom" => &self.detector,
            _ => "det_500m.onnx",
        }
    }

    pub fn recognizer(&self) -> &str {
        match self.level.as_str() {
            "low" | "medium" => "w600k_mbf.onnx",
            "high" | "maximum" => "w600k_r50.onnx",
            "custom" => &self.recognizer,
            _ => "w600k_mbf.onnx",
        }
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
    false
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
    #[serde(default = "default_dark_threshold")]
    pub dark_threshold: f64,
    #[serde(default = "default_dark_pixel_value")]
    pub dark_pixel_value: u8,
}

fn default_rgb_device() -> String {
    DEFAULT_RGB_CAMERA.to_string()
}

fn default_dark_threshold() -> f64 {
    0.6
}

fn default_dark_pixel_value() -> u8 {
    10
}

#[derive(Deserialize, Serialize, Clone, Debug, Value, OwnedValue, Type)]
pub struct AuthConfig {
    #[serde(default = "default_true")]
    pub abort_if_ssh: bool,
    #[serde(default = "default_true")]
    pub abort_if_lid_closed: bool,
    #[serde(default = "default_false")]
    pub require_confirmation: bool,
}

fn default_false() -> bool {
    false
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
        }
    }
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            rgb: default_rgb_device(),
            dark_threshold: default_dark_threshold(),
            dark_pixel_value: default_dark_pixel_value(),
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        Self::load_from(DEFAULT_CONFIG_PATH)
    }

    pub fn load_from(path: &str) -> anyhow::Result<Self> {
        if Path::new(path).exists() {
            let contents = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(DEFAULT_CONFIG_PATH)
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
            "custom-det.onnx".to_string(),
            "custom-rec.onnx".to_string(),
            0.73,
        );
        assert_eq!(custom.detector(), "custom-det.onnx");
        assert_eq!(custom.recognizer(), "custom-rec.onnx");
        assert!((custom.threshold() - 0.73).abs() < f32::EPSILON);
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
        assert!((config.cameras.dark_threshold - 0.6).abs() < f64::EPSILON);
        assert_eq!(config.cameras.dark_pixel_value, 10);
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert_eq!(config.enrollment.max_templates, 2);
    }

    #[test]
    fn save_to_and_load_from_round_trip() {
        let temp = TempDir::new("round-trip");
        let path = temp.path().join("config.toml");
        let config = Config {
            security: SecurityLevel::high(),
            cameras: CameraConfig {
                rgb: "primary".to_string(),
                dark_threshold: 0.75,
                dark_pixel_value: 8,
            },
            auth: AuthConfig {
                abort_if_ssh: true,
                abort_if_lid_closed: false,
                require_confirmation: true,
            },
            enrollment: EnrollmentConfig { max_templates: 8 },
            liveness: LivenessConfig::default(),
        };

        config.save_to(path.to_str().unwrap()).unwrap();
        let loaded = Config::load_from(path.to_str().unwrap()).unwrap();

        assert_eq!(loaded.security.detector(), SecurityLevel::high().detector());
        assert_eq!(
            loaded.security.recognizer(),
            SecurityLevel::high().recognizer()
        );
        assert_eq!(loaded.cameras.rgb, "primary");
        assert!((loaded.cameras.dark_threshold - 0.75).abs() < f64::EPSILON);
        assert_eq!(loaded.cameras.dark_pixel_value, 8);
        assert!(loaded.auth.abort_if_ssh);
        assert!(!loaded.auth.abort_if_lid_closed);
        assert!(loaded.auth.require_confirmation);
        assert_eq!(loaded.enrollment.max_templates, 8);
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
        assert!((config.cameras.dark_threshold - 0.6).abs() < f64::EPSILON);
        assert_eq!(config.cameras.dark_pixel_value, 10);
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert!(!config.auth.require_confirmation);
        assert_eq!(config.enrollment.max_templates, 2);
    }
}
