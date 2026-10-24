// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_SECURITY_THRESHOLD, HYBRID_POLICY_OPTIONS, MAX_SECURITY_THRESHOLD,
    MIN_SECURITY_THRESHOLD, MODEL_QUALITY_OPTIONS, SECURITY_LEVEL_OPTIONS, index_of, value_at,
};

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

#[derive(Serialize, Clone, Debug)]
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
                tracing::warn!("Invalid security level {other:?}; using medium detector");
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
                tracing::warn!("Invalid security level {other:?}; using medium recognizer");
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

    /// Returns every problem with the current values. `validate` reports only the
    /// first one, so diagnostics that need a complete list should use this method.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

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
