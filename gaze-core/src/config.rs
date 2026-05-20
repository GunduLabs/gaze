use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use zvariant::{OwnedValue, Value};

const DEFAULT_CONFIG_PATH: &str = "/etc/gaze/config.toml";
pub const USERS_DIR: &str = "/var/lib/gaze/users";
pub const MODELS_DIR: &str = "/var/cache/gaze";
pub const DEFAULT_RGB_CAMERA: &str = "primary";

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
    pub auth: AuthConfig,
    #[serde(default)]
    pub enrollment: EnrollmentConfig,
    #[serde(default)]
    pub liveness: LivenessConfig,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct LivenessConfig {
    #[serde(default = "default_liveness_enabled")]
    pub enabled: bool,
    #[serde(default = "default_liveness_threshold")]
    pub threshold: f32,
    #[serde(default = "default_max_frames")]
    pub max_frames: u32,
}

fn default_liveness_enabled() -> bool {
    false
}
fn default_liveness_threshold() -> f32 {
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

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct CameraConfig {
    #[serde(default = "default_rgb_device")]
    pub rgb: String,
    #[serde(default = "default_dark_threshold")]
    pub dark_threshold: f32,
    #[serde(default = "default_dark_pixel_value")]
    pub dark_pixel_value: u8,
}

fn default_rgb_device() -> String {
    DEFAULT_RGB_CAMERA.to_string()
}

fn default_dark_threshold() -> f32 {
    0.6
}

fn default_dark_pixel_value() -> u8 {
    10
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct AuthConfig {
    #[serde(default = "default_true")]
    pub abort_if_ssh: bool,
    #[serde(default = "default_true")]
    pub abort_if_lid_closed: bool,
}

fn default_true() -> bool {
    true
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

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            abort_if_ssh: true,
            abort_if_lid_closed: true,
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
        cameras.insert(
            "dark-threshold".to_string(),
            OwnedValue::try_from(Value::from(self.cameras.dark_threshold as f64)).unwrap(),
        );
        cameras.insert(
            "dark-pixel-value".to_string(),
            OwnedValue::try_from(Value::from(self.cameras.dark_pixel_value)).unwrap(),
        );
        map.insert("cameras".to_string(), cameras);

        let mut auth = HashMap::new();
        auth.insert(
            "abort-if-ssh".to_string(),
            OwnedValue::try_from(Value::from(self.auth.abort_if_ssh)).unwrap(),
        );
        auth.insert(
            "abort-if-lid-closed".to_string(),
            OwnedValue::try_from(Value::from(self.auth.abort_if_lid_closed)).unwrap(),
        );
        map.insert("auth".to_string(), auth);

        let mut enrollment = HashMap::new();
        enrollment.insert(
            "max-templates".to_string(),
            OwnedValue::try_from(Value::from(self.enrollment.max_templates)).unwrap(),
        );
        map.insert("enrollment".to_string(), enrollment);

        let mut liveness = HashMap::new();
        liveness.insert(
            "enabled".to_string(),
            OwnedValue::try_from(Value::from(self.liveness.enabled)).unwrap(),
        );
        liveness.insert(
            "threshold".to_string(),
            OwnedValue::try_from(Value::from(self.liveness.threshold as f64)).unwrap(),
        );
        liveness.insert(
            "max-frames".to_string(),
            OwnedValue::try_from(Value::from(self.liveness.max_frames)).unwrap(),
        );
        map.insert("liveness".to_string(), liveness);

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
                .unwrap_or_else(default_rgb_device),
            dark_threshold: cameras_dict
                .get("dark-threshold")
                .and_then(|v| {
                    let f: f64 = v.clone().try_into().ok()?;
                    Some(f as f32)
                })
                .unwrap_or_else(default_dark_threshold),
            dark_pixel_value: cameras_dict
                .get("dark-pixel-value")
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or_else(default_dark_pixel_value),
        };

        let auth = map
            .get("auth")
            .map_or_else(AuthConfig::default, |auth_dict| AuthConfig {
                abort_if_ssh: auth_dict
                    .get("abort-if-ssh")
                    .and_then(|v| v.clone().try_into().ok())
                    .unwrap_or(true),
                abort_if_lid_closed: auth_dict
                    .get("abort-if-lid-closed")
                    .and_then(|v| v.clone().try_into().ok())
                    .unwrap_or(true),
            });

        let enrollment_dict = map
            .get("enrollment")
            .context("missing enrollment section")?;
        let enrollment = EnrollmentConfig {
            max_templates: enrollment_dict
                .get("max-templates")
                .and_then(|v| v.clone().try_into().ok())
                .unwrap_or(2),
        };

        let liveness = match map.get("liveness") {
            Some(d) => LivenessConfig {
                enabled: d
                    .get("enabled")
                    .and_then(|v| v.clone().try_into().ok())
                    .unwrap_or_else(default_liveness_enabled),
                threshold: d
                    .get("threshold")
                    .and_then(|v| {
                        let f: f64 = v.clone().try_into().ok()?;
                        Some(f as f32)
                    })
                    .unwrap_or_else(default_liveness_threshold),
                max_frames: d
                    .get("max-frames")
                    .and_then(|v| v.clone().try_into().ok())
                    .unwrap_or_else(default_max_frames),
            },
            None => LivenessConfig::default(),
        };

        Ok(Self {
            security,
            cameras,
            auth,
            enrollment,
            liveness,
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

    fn owned_string(value: &OwnedValue) -> String {
        value.clone().try_into().unwrap()
    }

    fn owned_u32(value: &OwnedValue) -> u32 {
        value.clone().try_into().unwrap()
    }

    #[test]
    fn security_level_mappings_are_stable() {
        let cases = [
            (SecurityLevel::Low, "det_500m.onnx", "w600k_mbf.onnx", 0.3),
            (
                SecurityLevel::Medium,
                "det_500m.onnx",
                "w600k_mbf.onnx",
                0.4,
            ),
            (SecurityLevel::High, "det_10g.onnx", "w600k_r50.onnx", 0.5),
            (
                SecurityLevel::Maximum,
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

        let custom = SecurityLevel::Custom {
            detector: "custom-det.onnx".to_string(),
            recognizer: "custom-rec.onnx".to_string(),
            threshold: 0.73,
        };
        assert_eq!(custom.detector(), "custom-det.onnx");
        assert_eq!(custom.recognizer(), "custom-rec.onnx");
        assert!((custom.threshold() - 0.73).abs() < f32::EPSILON);
    }

    #[test]
    fn config_map_round_trips_custom_values() {
        let config = Config {
            security: SecurityLevel::Custom {
                detector: "det_custom.onnx".to_string(),
                recognizer: "rec_custom.onnx".to_string(),
                threshold: 0.77,
            },
            cameras: CameraConfig {
                rgb: "pipewiresrc target-object=42".to_string(),
                dark_threshold: 0.7,
                dark_pixel_value: 12,
            },
            auth: AuthConfig {
                abort_if_ssh: false,
                abort_if_lid_closed: true,
            },
            enrollment: EnrollmentConfig { max_templates: 5 },
            liveness: LivenessConfig {
                enabled: true,
                threshold: 0.85,
                max_frames: 30,
            },
        };

        let map = config.to_map();
        assert_eq!(owned_string(&map["security"]["level"]), "custom");
        assert_eq!(
            owned_string(&map["security"]["detector"]),
            "det_custom.onnx"
        );
        assert_eq!(
            owned_string(&map["security"]["recognizer"]),
            "rec_custom.onnx"
        );
        assert_eq!(
            owned_string(&map["cameras"]["rgb"]),
            "pipewiresrc target-object=42"
        );
        let dark_threshold: f64 = map["cameras"]["dark-threshold"].clone().try_into().unwrap();
        let dark_pixel_value: u8 = map["cameras"]["dark-pixel-value"]
            .clone()
            .try_into()
            .unwrap();
        assert!((dark_threshold - 0.7).abs() < 1e-6);
        assert_eq!(dark_pixel_value, 12);
        let abort_if_ssh: bool = map["auth"]["abort-if-ssh"].clone().try_into().unwrap();
        let abort_if_lid_closed: bool = map["auth"]["abort-if-lid-closed"]
            .clone()
            .try_into()
            .unwrap();
        assert!(!abort_if_ssh);
        assert!(abort_if_lid_closed);
        assert_eq!(owned_u32(&map["enrollment"]["max-templates"]), 5);
        let liveness_enabled: bool = map["liveness"]["enabled"].clone().try_into().unwrap();
        assert!(liveness_enabled);
        assert_eq!(owned_u32(&map["liveness"]["max-frames"]), 30);

        let decoded = Config::from_map(map).unwrap();
        match decoded.security {
            SecurityLevel::Custom {
                detector,
                recognizer,
                threshold,
            } => {
                assert_eq!(detector, "det_custom.onnx");
                assert_eq!(recognizer, "rec_custom.onnx");
                assert!((threshold - 0.77).abs() < f32::EPSILON);
            }
            other => panic!("unexpected security level: {other:?}"),
        }
        assert!(decoded.liveness.enabled);
        assert!((decoded.liveness.threshold - 0.85).abs() < 1e-5);
        assert_eq!(decoded.liveness.max_frames, 30);
        assert_eq!(decoded.cameras.rgb, "pipewiresrc target-object=42");
        assert!((decoded.cameras.dark_threshold - 0.7).abs() < f32::EPSILON);
        assert_eq!(decoded.cameras.dark_pixel_value, 12);
        assert!(!decoded.auth.abort_if_ssh);
        assert!(decoded.auth.abort_if_lid_closed);
        assert_eq!(decoded.enrollment.max_templates, 5);
    }

    #[test]
    fn from_map_defaults_missing_optional_values() {
        let mut map = HashMap::new();
        let mut security = HashMap::new();
        security.insert(
            "level".to_string(),
            OwnedValue::try_from(Value::from("custom")).unwrap(),
        );
        map.insert("security".to_string(), security);
        map.insert("cameras".to_string(), HashMap::new());
        map.insert("enrollment".to_string(), HashMap::new());

        let config = Config::from_map(map).unwrap();
        match config.security {
            SecurityLevel::Custom {
                detector,
                recognizer,
                threshold,
            } => {
                assert_eq!(detector, "det_10g.onnx");
                assert_eq!(recognizer, "w600k_r50.onnx");
                assert!((threshold - 0.6).abs() < f32::EPSILON);
            }
            other => panic!("unexpected security level: {other:?}"),
        }
        assert_eq!(config.cameras.rgb, DEFAULT_RGB_CAMERA);
        assert!((config.cameras.dark_threshold - 0.6).abs() < f32::EPSILON);
        assert_eq!(config.cameras.dark_pixel_value, 10);
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert_eq!(config.enrollment.max_templates, 2);
    }

    #[test]
    fn from_map_requires_all_sections() {
        let err = Config::from_map(HashMap::new()).unwrap_err();
        assert!(err.to_string().contains("missing security section"));

        let map = Config::default().to_map();
        for section in ["security", "cameras", "enrollment"] {
            let mut missing = map.clone();
            missing.remove(section);
            let err = Config::from_map(missing).unwrap_err();
            assert!(
                err.to_string()
                    .contains(&format!("missing {section} section"))
            );
        }
    }

    #[test]
    fn unknown_security_level_falls_back_to_medium() {
        let mut map = Config::default().to_map();
        map.get_mut("security").unwrap().insert(
            "level".to_string(),
            OwnedValue::try_from(Value::from("paranoid")).unwrap(),
        );

        let config = Config::from_map(map).unwrap();
        assert_eq!(config.security.detector(), SecurityLevel::Medium.detector());
        assert_eq!(
            config.security.recognizer(),
            SecurityLevel::Medium.recognizer()
        );
        assert!(
            (config.security.threshold() - SecurityLevel::Medium.threshold()).abs() < f32::EPSILON
        );
    }

    #[test]
    fn load_from_missing_file_returns_default() {
        let temp = TempDir::new("missing");
        let path = temp.path().join("missing.toml");

        let config = Config::load_from(path.to_str().unwrap()).unwrap();
        assert_eq!(config.security.detector(), SecurityLevel::Medium.detector());
        assert_eq!(config.cameras.rgb, DEFAULT_RGB_CAMERA);
        assert!((config.cameras.dark_threshold - 0.6).abs() < f32::EPSILON);
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
            security: SecurityLevel::High,
            cameras: CameraConfig {
                rgb: "primary".to_string(),
                dark_threshold: 0.75,
                dark_pixel_value: 8,
            },
            auth: AuthConfig {
                abort_if_ssh: true,
                abort_if_lid_closed: false,
            },
            enrollment: EnrollmentConfig { max_templates: 8 },
            liveness: LivenessConfig::default(),
        };

        config.save_to(path.to_str().unwrap()).unwrap();
        let loaded = Config::load_from(path.to_str().unwrap()).unwrap();

        assert_eq!(loaded.security.detector(), SecurityLevel::High.detector());
        assert_eq!(
            loaded.security.recognizer(),
            SecurityLevel::High.recognizer()
        );
        assert_eq!(loaded.cameras.rgb, "primary");
        assert!((loaded.cameras.dark_threshold - 0.75).abs() < f32::EPSILON);
        assert_eq!(loaded.cameras.dark_pixel_value, 8);
        assert!(loaded.auth.abort_if_ssh);
        assert!(!loaded.auth.abort_if_lid_closed);
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
            SecurityLevel::Maximum.detector()
        );
        assert_eq!(config.cameras.rgb, DEFAULT_RGB_CAMERA);
        assert!((config.cameras.dark_threshold - 0.6).abs() < f32::EPSILON);
        assert_eq!(config.cameras.dark_pixel_value, 10);
        assert!(config.auth.abort_if_ssh);
        assert!(config.auth.abort_if_lid_closed);
        assert_eq!(config.enrollment.max_templates, 2);
    }
}
