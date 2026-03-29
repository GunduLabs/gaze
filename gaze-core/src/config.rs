use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use zvariant::{OwnedValue, Value};

const DEFAULT_CONFIG_PATH: &str = "/etc/gaze/config.toml";
pub const USERS_DIR: &str = "/var/lib/gaze/users";
pub const MODELS_DIR: &str = "/var/cache/gaze";

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
#[serde(tag = "level", rename_all = "kebab-case")]
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

#[derive(Deserialize, Serialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub security: SecurityLevel,
    #[serde(default)]
    pub cameras: CameraConfig,
    #[serde(default)]
    pub enrollment: EnrollmentConfig,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CameraConfig {
    #[serde(default = "default_rgb_device")]
    pub rgb: String,
}

fn default_rgb_device() -> String {
    "/dev/video0".to_string()
}

#[derive(Deserialize, Serialize, Clone, Debug)]
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
    pub fn to_map(&self) -> HashMap<String, HashMap<String, OwnedValue>> {
        let mut map = HashMap::new();

        let mut security = HashMap::new();
        match &self.security {
            SecurityLevel::Low => {
                security.insert(
                    "level".into(),
                    OwnedValue::try_from(Value::from("low")).unwrap(),
                );
            }
            SecurityLevel::Medium => {
                security.insert(
                    "level".into(),
                    OwnedValue::try_from(Value::from("medium")).unwrap(),
                );
            }
            SecurityLevel::High => {
                security.insert(
                    "level".into(),
                    OwnedValue::try_from(Value::from("high")).unwrap(),
                );
            }
            SecurityLevel::Maximum => {
                security.insert(
                    "level".into(),
                    OwnedValue::try_from(Value::from("maximum")).unwrap(),
                );
            }
            SecurityLevel::Custom {
                detector,
                recognizer,
                threshold,
            } => {
                security.insert(
                    "level".into(),
                    OwnedValue::try_from(Value::from("custom")).unwrap(),
                );
                security.insert(
                    "detector".into(),
                    OwnedValue::try_from(Value::from(detector.clone())).unwrap(),
                );
                security.insert(
                    "recognizer".into(),
                    OwnedValue::try_from(Value::from(recognizer.clone())).unwrap(),
                );
                security.insert(
                    "threshold".into(),
                    OwnedValue::try_from(Value::from(*threshold as f64)).unwrap(),
                );
            }
        }
        map.insert("security".to_string(), security);

        let mut cameras = HashMap::new();
        cameras.insert(
            "rgb".to_string(),
            OwnedValue::try_from(Value::from(self.cameras.rgb.clone())).unwrap(),
        );
        map.insert("cameras".to_string(), cameras);

        let mut enrollment = HashMap::new();
        enrollment.insert(
            "max-templates".to_string(),
            OwnedValue::try_from(Value::from(self.enrollment.max_templates)).unwrap(),
        );
        map.insert("enrollment".to_string(), enrollment);

        map
    }

    pub fn from_map(map: HashMap<String, HashMap<String, OwnedValue>>) -> anyhow::Result<Self> {
        let security_dict = map.get("security").context("missing security section")?;
        let level_str: String = security_dict
            .get("level")
            .and_then(|v| v.clone().try_into().ok())
            .unwrap_or_else(|| "medium".to_string());

        let security = match level_str.as_str() {
            "low" => SecurityLevel::Low,
            "medium" => SecurityLevel::Medium,
            "high" => SecurityLevel::High,
            "maximum" => SecurityLevel::Maximum,
            "custom" => {
                let detector = security_dict
                    .get("detector")
                    .and_then(|v| v.clone().try_into().ok())
                    .unwrap_or_else(|| "det_10g.onnx".to_string());
                let recognizer = security_dict
                    .get("recognizer")
                    .and_then(|v| v.clone().try_into().ok())
                    .unwrap_or_else(|| "w600k_r50.onnx".to_string());
                let threshold: f32 = security_dict
                    .get("threshold")
                    .and_then(|v| {
                        let f: f64 = v.clone().try_into().ok()?;
                        Some(f as f32)
                    })
                    .unwrap_or(0.6);
                SecurityLevel::Custom {
                    detector,
                    recognizer,
                    threshold,
                }
            }
            _ => SecurityLevel::Medium,
        };

        let cameras_dict = map.get("cameras").context("missing cameras section")?;
        let cameras = CameraConfig {
            rgb: cameras_dict
                .get("rgb")
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or_else(|| "/dev/video0".to_string()),
        };

        let enrollment_dict = map
            .get("enrollment")
            .context("missing enrollment section")?;
        let enrollment = EnrollmentConfig {
            max_templates: enrollment_dict
                .get("max-templates")
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or(2),
        };

        Ok(Self {
            security,
            cameras,
            enrollment,
        })
    }

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
