// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[test]
fn config_keys_round_trip_through_get_and_set() {
    let config = Config::default();
    let enabled = config.liveness.enabled;
    assert_eq!(
        config.get_key("liveness.enabled").unwrap(),
        enabled.to_string()
    );
    let changed = config
        .with_key("liveness.enabled", &(!enabled).to_string())
        .unwrap();
    assert_eq!(changed.liveness.enabled, !enabled);
    let changed = config
        .with_key("cameras.dark_luma_threshold", "42")
        .unwrap();
    assert_eq!(changed.cameras.dark_luma_threshold, 42);
    assert!(
        config
            .with_key("cameras.dark_luma_threshold", "300")
            .is_err()
    );
    assert!(config.with_key("liveness.enabled", "maybe").is_err());
    assert!(config.with_key("liveness.nonsense", "1").is_err());
    assert!(config.with_key("liveness", "1").is_err());
    assert!(config.get_key("liveness").is_err());
    assert!(config.to_toml().unwrap().contains("[duress]"));
    assert!(config.with_key("liveness.ir_model", "true").is_err());
    assert!(config.with_key("liveness.ir_threshold", "0.6").is_err());
}
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
fn an_existing_config_below_the_minimum_seconds_falls_back_to_the_default() {
    for stale in [0.0, 0.05, 0.1, 0.19] {
        let liveness = LivenessConfig {
            max_seconds: stale,
            ..LivenessConfig::default()
        };
        assert!(liveness.validate().is_err(), "{stale} must be rejected");
        assert_eq!(
            liveness.effective_max_seconds(),
            DEFAULT_LIVENESS_MAX_SECONDS,
            "{stale} must fall back to the default rather than gate auth"
        );
    }
}

#[test]
fn effective_max_frames_scales_with_camera_fps() {
    let liveness = LivenessConfig {
        max_seconds: 2.0,
        ..LivenessConfig::default()
    };
    assert_eq!(liveness.effective_max_frames(30.0), 60);
    assert_eq!(liveness.effective_max_frames(60.0), 120);
    assert_eq!(liveness.effective_max_frames(15.0), 30);
    // Even a short duration still allows the minimum number of frames.
    let short_liveness = LivenessConfig {
        max_seconds: 0.2,
        ..LivenessConfig::default()
    };
    assert_eq!(
        short_liveness.effective_max_frames(10.0),
        MIN_LIVENESS_FRAMES
    );
}

#[test]
fn the_smallest_allowed_frame_budget_can_reach_sustained_liveness() {
    const {
        assert!(
            MIN_LIVENESS_FRAMES > 5,
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
    let config: Config = toml_edit::de::from_str(
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

    let migrated = toml_edit::ser::to_string(&config).unwrap();
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
    let config: Config = toml_edit::de::from_str(
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

    cfg.max_seconds = 0.0;
    assert!(cfg.validate().is_err());
    assert_eq!(cfg.effective_max_seconds(), DEFAULT_LIVENESS_MAX_SECONDS);

    cfg.max_seconds = 2.5;
    cfg.validate().unwrap();
    assert_eq!(cfg.effective_max_seconds(), 2.5);

    for bad in [0.0, 1.01, f64::NAN] {
        cfg.ir_threshold = bad;
        assert!(cfg.validate().is_err(), "{bad}");
        assert_eq!(cfg.effective_ir_threshold(), DEFAULT_IR_LIVENESS_THRESHOLD);
    }
    cfg.ir_threshold = 0.7;
    cfg.validate().unwrap();
    assert_eq!(cfg.effective_ir_threshold(), 0.7);
}

#[test]
fn ir_model_is_opt_in_and_needs_liveness() {
    let mut cfg = LivenessConfig::default();
    assert!(!cfg.ir_model);
    assert!(!cfg.ir_model_enabled());
    cfg.ir_model = true;
    assert!(cfg.ir_model_enabled());
    cfg.enabled = false;
    assert!(!cfg.ir_model_enabled());

    let config: Config = toml_edit::de::from_str("[liveness]\nenabled = true\n").unwrap();
    assert!(!config.liveness.ir_model);
    assert_eq!(config.liveness.ir_threshold, DEFAULT_IR_LIVENESS_THRESHOLD);
    assert!(unknown_config_keys("[liveness]\nir_model = true\nir_threshold = 0.6\n").is_empty());
}

#[test]
fn load_from_falls_back_on_a_degenerate_custom_threshold() {
    let temp = TempDir::new("bad-threshold");
    let path = temp.path().join("config.toml");
    std::fs::write(
            &path,
            "[security]\nlevel = \"custom\"\nthreshold = 0.0\n\n[liveness]\nthreshold = 0.0\nmax_seconds = 0.0\n",
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
    assert_eq!(
        config.liveness.effective_max_seconds(),
        DEFAULT_LIVENESS_MAX_SECONDS
    );
}

#[test]
fn load_from_migrates_legacy_max_frames() {
    // Case 1: default 40 frames migrates to 2.0s.
    let temp = TempDir::new("legacy-max-frames-40");
    let path = temp.path().join("config.toml");
    let legacy = "[liveness]\nenabled = true\nthreshold = 0.85\nmax_frames = 40\n";
    std::fs::write(&path, legacy).unwrap();

    let config = Config::load_from(path.to_str().unwrap()).unwrap();
    assert_eq!(config.liveness.max_seconds, 2.0);
    assert_eq!(config.liveness.threshold, 0.85);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), legacy);

    Config::migrate_file(path.to_str().unwrap());
    let on_disk = std::fs::read_to_string(&path).unwrap();
    assert!(on_disk.contains("max_seconds = 2.0"));
    assert!(!on_disk.contains("max_frames"));

    // Case 2: custom non-40 frames migrates faithfully (e.g. 25 frames -> 0.83s).
    let temp2 = TempDir::new("legacy-max-frames-custom");
    let path2 = temp2.path().join("config.toml");
    std::fs::write(
        &path2,
        "[liveness]\nenabled = true\nthreshold = 0.8\nmax_frames = 25\n",
    )
    .unwrap();

    let config2 = Config::load_from(path2.to_str().unwrap()).unwrap();
    assert_eq!(config2.liveness.max_seconds, 0.83);

    Config::migrate_file(path2.to_str().unwrap());
    let on_disk2 = std::fs::read_to_string(&path2).unwrap();
    assert!(on_disk2.contains("max_seconds = 0.83"));
    assert!(!on_disk2.contains("max_frames"));
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
    let supported = [
        ("cpu", "cpu"),
        ("openvino", "cpu"),
        ("openvino", "gpu"),
        ("openvino", "npu"),
        ("auto", "npu"),
        ("vitis", "npu"),
    ];

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
    let invalid = [
        ("cpu", "gpu"),
        ("cpu", "npu"),
        ("openvino", "cuda"),
        ("webgpu", "gpu"),
        ("auto", "cpu"),
        ("auto", "gpu"),
        ("vitis", "cpu"),
        ("vitis", "gpu"),
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
    assert!(openvino.is_representable());
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
    let contents = include_str!("../../../../packaging/config/config.toml");
    assert!(unknown_config_keys(contents).is_empty());
}

#[test]
fn the_dbus_signature_is_unchanged() {
    use zbus::Type;
    assert_eq!(
        crate::dbus::DbusConfig::SIGNATURE.to_string(),
        "((ss)(sssdds)(ssbys)(bbbbbtts)(ud)(bdd)(b))"
    );
}

#[test]
fn legacy_require_confirmation_applies_to_both_surfaces() {
    let auth: AuthConfig = toml_edit::de::from_str("require_confirmation = true\n").unwrap();
    assert!(auth.require_confirmation_lock_screen);
    assert!(auth.require_confirmation_elevation);
    assert!(auth.requires_confirmation(AuthSurface::Elevation));
    assert!(auth.requires_confirmation(AuthSurface::ScreenLock));
}

#[test]
fn an_explicit_split_key_overrides_the_legacy_key() {
    let auth: AuthConfig = toml_edit::de::from_str(
        "require_confirmation = true\nrequire_confirmation_elevation = false\n",
    )
    .unwrap();
    assert!(auth.require_confirmation_lock_screen);
    assert!(!auth.require_confirmation_elevation);
}

#[test]
fn confirmation_defaults_to_off_without_any_key() {
    let auth: AuthConfig = toml_edit::de::from_str("").unwrap();
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
    assert!(config.storage.encrypt_templates);
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
    let original = include_str!("../../../../packaging/config/config.toml");
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
    assert!(!config.auth.abort_before_first_resume);
    assert_eq!(config.enrollment.max_templates, 2);
    assert_eq!(
        config.enrollment.min_face_size_ratio,
        DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO
    );
    assert!(config.storage.encrypt_templates);
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
            ir_frame_width: Some(340),
            ir_frame_height: Some(340),
        },
        auth: AuthConfig {
            abort_if_ssh: true,
            abort_if_lid_closed: false,
            abort_before_first_resume: true,
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
            max_seconds: 2.5,
            ir_model: true,
            ir_threshold: 0.6,
        },
        storage: StorageConfig {
            encrypt_templates: true,
            unlock_gnome_keyring: true,
            unlock_kwallet: true,
        },
        duress: DuressConfig {
            enabled: true,
            closed_threshold: 0.8,
            hold_ms: 900,
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
    assert_eq!(loaded.cameras.ir_frame_size(), Some((340, 340)));
    assert!(loaded.auth.abort_if_ssh);
    assert!(!loaded.auth.abort_if_lid_closed);
    assert!(loaded.auth.abort_before_first_resume);
    assert!(loaded.auth.require_confirmation_lock_screen);
    assert!(loaded.auth.require_confirmation_elevation);
    assert_eq!(loaded.auth.resume_grace_ms, 3000);
    assert_eq!(loaded.auth.start_delay_ms, 1500);
    assert_eq!(loaded.auth.start_delay_scope(), "screen_lock");
    assert_eq!(loaded.enrollment.max_templates, 8);
    assert_eq!(loaded.enrollment.min_face_size_ratio, 0.20);
    assert!(loaded.liveness.enabled);
    assert_eq!(loaded.liveness.threshold, 0.9);
    assert_eq!(loaded.liveness.max_seconds, 2.5);
    assert!(loaded.liveness.ir_model);
    assert_eq!(loaded.liveness.ir_threshold, 0.6);
    assert!(loaded.storage.encrypt_templates);
    assert!(loaded.storage.unlock_gnome_keyring);
    assert!(loaded.storage.unlock_kwallet);
    assert!(loaded.duress.enabled);
    assert_eq!(loaded.duress.closed_threshold, 0.8);
    assert_eq!(loaded.duress.hold_ms, 900);
}

#[test]
fn duress_is_off_by_default_and_falls_back_on_invalid_values() {
    let duress = Config::default().duress;
    assert!(!duress.enabled);
    assert!(duress.validate().is_ok());
    assert_eq!(
        duress.effective_closed_threshold(),
        DEFAULT_DURESS_CLOSED_THRESHOLD as f32
    );

    for closed_threshold in [0.1, 1.5, f64::NAN] {
        let bad = DuressConfig {
            closed_threshold,
            ..DuressConfig::default()
        };
        assert!(bad.validate().is_err());
        assert_eq!(
            bad.effective_closed_threshold(),
            DEFAULT_DURESS_CLOSED_THRESHOLD as f32
        );
    }

    let long_hold = DuressConfig {
        hold_ms: MAX_DURESS_HOLD_MS + 1,
        ..DuressConfig::default()
    };
    assert!(long_hold.validate().is_err());
    assert_eq!(
        long_hold.effective_hold(),
        std::time::Duration::from_millis(MAX_DURESS_HOLD_MS)
    );
}

#[test]
fn ir_camera_fields_default_empty_and_disabled() {
    let config: Config = toml_edit::de::from_str(
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
    let config: Config = toml_edit::de::from_str(
        r#"
            [liveness]
            enabled = true
            "#,
    )
    .unwrap();

    assert!(config.liveness.enabled);
    assert!((config.liveness.threshold - 0.8).abs() < f64::EPSILON);
    assert_eq!(config.liveness.max_seconds, DEFAULT_LIVENESS_MAX_SECONDS);
}

#[test]
fn partial_toml_uses_serde_defaults() {
    let config: Config = toml_edit::de::from_str(
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
    assert!(!config.auth.abort_before_first_resume);
    assert!(!config.auth.require_confirmation_lock_screen);
    assert!(!config.auth.require_confirmation_elevation);
    assert_eq!(config.auth.start_delay_ms, 0);
    assert_eq!(config.enrollment.max_templates, 2);
    assert_eq!(
        config.enrollment.min_face_size_ratio,
        DEFAULT_ENROLLMENT_MIN_FACE_SIZE_RATIO
    );
    assert!(config.storage.encrypt_templates);
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

    let value = zbus::OwnedVariant::try_from(crate::dbus::DbusConfig::from(cfg)).unwrap();
    let back = crate::dbus::DbusConfig::try_from(value)
        .map(Config::from)
        .unwrap();

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
fn storage_encrypt_templates_defaults_true_but_respects_explicit_opt_out() {
    let enabled: Config = toml_edit::de::from_str(
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

    let absent: Config = toml_edit::de::from_str(
        r#"[security]
level = "low""#,
    )
    .unwrap();
    assert!(absent.storage.encrypt_templates);
    assert!(StorageConfig::default().encrypt_templates);
    let empty_table: Config = toml_edit::de::from_str("[storage]").unwrap();
    assert!(empty_table.storage.encrypt_templates);
    let disabled: Config = toml_edit::de::from_str("[storage]\nencrypt_templates = false").unwrap();
    assert!(!disabled.storage.encrypt_templates);
}

#[test]
fn kwallet_is_independent_and_clamped_without_prerequisites() {
    let mut config = Config::default();
    assert!(!config.storage.unlock_kwallet);
    assert!(unknown_config_keys("[storage]\nunlock_kwallet = true").is_empty());
    for (encrypted, live) in [(false, true), (true, false), (false, false), (true, true)] {
        config.storage.unlock_kwallet = true;
        config.storage.encrypt_templates = encrypted;
        config.liveness.enabled = live;
        assert_eq!(
            config.storage.validate_keyring(&config.liveness).is_ok(),
            encrypted && live
        );
        assert_eq!(config.clamp_keyring(), !(encrypted && live));
        assert_eq!(config.storage.unlock_kwallet, encrypted && live);
        assert!(!config.storage.unlock_gnome_keyring);
    }
}

#[test]
fn keyring_defaults_off_and_requires_tpm_and_liveness() {
    let mut config: Config =
        toml_edit::de::from_str("[storage]\nencrypt_templates = true").unwrap();
    assert!(!config.storage.unlock_gnome_keyring);
    config.storage.unlock_gnome_keyring = true;
    config.liveness.enabled = true;
    assert!(config.storage.validate_keyring(&config.liveness).is_ok());
    config.storage.encrypt_templates = false;
    assert!(config.storage.validate_keyring(&config.liveness).is_err());
    config.storage.encrypt_templates = true;
    config.liveness.enabled = false;
    assert!(config.storage.validate_keyring(&config.liveness).is_err());
    config.storage.unlock_gnome_keyring = false;
    assert!(config.storage.validate_keyring(&config.liveness).is_ok());
    assert!(unknown_config_keys("[storage]\nunlock_gnome_keyring = false").is_empty());
}

#[test]
fn a_hand_edited_keyring_opt_in_without_prerequisites_clamps_off() {
    let mut config = Config::default();
    config.storage.unlock_gnome_keyring = true;
    config.storage.encrypt_templates = true;
    config.liveness.enabled = false;
    assert!(config.clamp_keyring());
    assert!(!config.storage.unlock_gnome_keyring);

    config.storage.unlock_gnome_keyring = true;
    config.liveness.enabled = true;
    assert!(!config.clamp_keyring(), "a valid opt-in survives");
    assert!(config.storage.unlock_gnome_keyring);

    let mut off = Config::default();
    assert!(!off.clamp_keyring(), "the default needs no clamping");
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
    assert_eq!(
        INFERENCE_EXECUTION_PROVIDER_OPTIONS.len(),
        INFERENCE_EXECUTION_PROVIDER_LABELS.len()
    );
    assert_eq!(
        INFERENCE_DEVICE_OPTIONS.len(),
        INFERENCE_DEVICE_LABELS.len()
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

#[test]
fn ir_frame_size_defaults_to_auto_negotiation() {
    let config: Config = toml_edit::de::from_str(
        r#"
            [cameras]
            rgb = "primary"
            "#,
    )
    .unwrap();

    assert_eq!(config.cameras.ir_frame_size(), None);
    config.cameras.validate().unwrap();
}

#[test]
fn explicit_ir_frame_size_is_parsed_as_a_known_key() {
    let contents = r#"
            [cameras]
            rgb = "primary"
            ir_frame_width = 340
            ir_frame_height = 340
            "#;
    let config: Config = toml_edit::de::from_str(contents).unwrap();

    config.cameras.validate().unwrap();
    assert_eq!(config.cameras.ir_frame_size(), Some((340, 340)));
    assert!(unknown_config_keys(contents).is_empty());
}

#[test]
fn validate_rejects_a_partial_or_out_of_range_ir_frame_size() {
    for (width, height) in [
        (Some(340), None),
        (None, Some(340)),
        (Some(0), Some(340)),
        (Some(340), Some(MAX_IR_FRAME_DIMENSION + 1)),
    ] {
        let cameras = CameraConfig {
            ir_frame_width: width,
            ir_frame_height: height,
            ..CameraConfig::default()
        };
        assert!(cameras.validate().is_err(), "{width:?}x{height:?}");
        assert_eq!(cameras.ir_frame_size(), None, "{width:?}x{height:?}");
    }
}

#[test]
fn save_to_omits_an_unset_ir_frame_size_and_removes_a_cleared_one() {
    let temp = TempDir::new("ir-frame-size");
    let path = temp.path().join("config.toml");
    let path = path.to_str().unwrap();

    Config::default().save_to(path).unwrap();
    assert!(!fs::read_to_string(path).unwrap().contains("ir_frame"));

    fs::write(
        path,
        "[cameras]\n# keep me\nir = \"/dev/video2\"\nir_frame_width = 340\nir_frame_height = 340\n",
    )
    .unwrap();
    let mut config = Config::load_from(path).unwrap();
    assert_eq!(config.cameras.ir_frame_size(), Some((340, 340)));

    config.cameras.set_ir_frame_size(None);
    config.save_to(path).unwrap();
    let saved = fs::read_to_string(path).unwrap();
    assert!(saved.contains("# keep me"), "{saved}");
    assert!(!saved.contains("ir_frame"), "{saved}");
    assert_eq!(
        Config::load_from(path).unwrap().cameras.ir_frame_size(),
        None
    );
}
