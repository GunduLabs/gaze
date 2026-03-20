use serde::Deserialize;
use std::path::Path;

const DEFAULT_CONFIG_PATH: &str = "/etc/gaze/config.toml";

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(tag = "level")]
pub enum SecurityLevel {
    #[serde(rename = "low")]
    Low,
    #[serde(rename = "medium")]
    #[default]
    Medium,
    #[serde(rename = "high")]
    High,
    #[serde(rename = "maximum")]
    Maximum,
    #[serde(rename = "custom")]
    Custom {
        detector: String,
        recognizer: String,
        threshold: f32,
    },
}

impl SecurityLevel {
    pub fn detector(&self) -> &str {
        match self {
            SecurityLevel::Low | SecurityLevel::Medium => "det_500m.onnx",
            SecurityLevel::High | SecurityLevel::Maximum => "det_10g.onnx",
            SecurityLevel::Custom { detector, .. } => detector,
        }
    }

    pub fn recognizer(&self) -> &str {
        match self {
            SecurityLevel::Low | SecurityLevel::Medium => "w600k_mbf.onnx",
            SecurityLevel::High | SecurityLevel::Maximum => "w600k_r50.onnx",
            SecurityLevel::Custom { recognizer, .. } => recognizer,
        }
    }

    pub fn threshold(&self) -> f32 {
        match self {
            SecurityLevel::Low => 0.3,
            SecurityLevel::Medium => 0.4,
            SecurityLevel::High => 0.5,
            SecurityLevel::Maximum => 0.6,
            SecurityLevel::Custom { threshold, .. } => *threshold,
        }
    }
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub security: SecurityLevel,
    #[serde(default)]
    pub cameras: CameraConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub enrollment: EnrollmentConfig,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CameraConfig {
    #[serde(default = "default_rgb_device")]
    pub rgb: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct StorageConfig {
    #[serde(default = "default_users_dir")]
    pub users_dir: String,
    #[serde(default = "default_models_dir")]
    pub models_dir: String,
}

fn default_rgb_device() -> String {
    "/dev/video0".to_string()
}
fn default_users_dir() -> String {
    "/var/lib/gaze/users".to_string()
}
fn default_models_dir() -> String {
    "/var/cache/gaze".to_string()
}
fn default_max_captures() -> usize {
    8
}

#[derive(Deserialize, Clone, Debug)]
pub struct EnrollmentConfig {
    #[serde(default = "default_max_captures")]
    pub max_captures_per_face: usize,
}

impl Default for EnrollmentConfig {
    fn default() -> Self {
        Self {
            max_captures_per_face: default_max_captures(),
        }
    }
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            rgb: default_rgb_device(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            users_dir: default_users_dir(),
            models_dir: default_models_dir(),
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
}
