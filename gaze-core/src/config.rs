use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::Path;

const DEFAULT_CONFIG_PATH: &str = "/etc/gaze/config.toml";
pub const USERS_DIR: &str = "/var/lib/gaze/users";
pub const MODELS_DIR: &str = "/var/cache/gaze";

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case", tag = "level")]
#[derive(Default)]
pub enum SecurityLevel {
    Low,
    #[default]
    Medium,
    High,
    Maximum,
    Custom {
        detector: String,
        recognizer: String,
        threshold: f32,
    },
}

impl zbus::zvariant::Type for SecurityLevel {
    fn signature() -> zbus::zvariant::Signature<'static> {
        zbus::zvariant::Signature::from_static_str_unchecked("v")
    }
}

impl std::fmt::Display for SecurityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SecurityLevel::Low => "low",
            SecurityLevel::Medium => "medium",
            SecurityLevel::High => "high",
            SecurityLevel::Maximum => "maximum",
            SecurityLevel::Custom { .. } => "custom",
        };
        write!(f, "{}", s)
    }
}

impl SecurityLevel {
    pub fn detector(&self) -> &str {
        match self {
            SecurityLevel::Low | SecurityLevel::Medium => "det_500m.onnx",
            SecurityLevel::High | SecurityLevel::Maximum => "det_10g.onnx",
            SecurityLevel::Custom { detector, .. } => {
                if detector.is_empty() {
                    "det_10g.onnx"
                } else {
                    detector
                }
            }
        }
    }

    pub fn recognizer(&self) -> &str {
        match self {
            SecurityLevel::Low | SecurityLevel::Medium => "w600k_mbf.onnx",
            SecurityLevel::High | SecurityLevel::Maximum => "w600k_r50.onnx",
            SecurityLevel::Custom { recognizer, .. } => {
                if recognizer.is_empty() {
                    "w600k_r50.onnx"
                } else {
                    recognizer
                }
            }
        }
    }

    pub fn threshold(&self) -> f32 {
        match self {
            SecurityLevel::Low => 0.3,
            SecurityLevel::Medium => 0.4,
            SecurityLevel::High => 0.5,
            SecurityLevel::Maximum => 0.6,
            SecurityLevel::Custom { threshold, .. } => {
                if *threshold <= 0.0 {
                    0.5
                } else {
                    *threshold
                }
            }
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, Default, zbus::zvariant::Type)]
pub struct Config {
    #[serde(default)]
    pub security: SecurityLevel,
    #[serde(default)]
    pub cameras: CameraConfig,
    #[serde(default)]
    pub enrollment: EnrollmentConfig,
}

#[derive(Deserialize, Serialize, Clone, Debug, zbus::zvariant::Type)]
pub struct CameraConfig {
    #[serde(default = "default_rgb_device")]
    pub rgb: String,
}

fn default_rgb_device() -> String {
    "/dev/video0".to_string()
}
#[derive(Deserialize, Serialize, Clone, Debug, zbus::zvariant::Type)]
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

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            rgb: default_rgb_device(),
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
        std::fs::write(path, encoded)
            .with_context(|| format!("failed to write config file: {}", path))?;
        Ok(())
    }
}
