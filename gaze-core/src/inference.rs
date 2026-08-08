// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::config::InferenceConfig;
use anyhow::Context;
#[cfg(feature = "openvino")]
use ort::ep;
use ort::session::{Session, builder::GraphOptimizationLevel};
use std::ffi::CStr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InferenceRuntime {
    pub requested_execution_provider: String,
    pub requested_device: String,
    pub active_execution_provider: String,
    pub active_device: String,
    pub fallback_reason: Option<String>,
}

impl InferenceRuntime {
    fn cpu(config: &InferenceConfig, fallback_reason: Option<String>) -> Self {
        Self {
            requested_execution_provider: config.execution_provider.clone(),
            requested_device: config.device.clone(),
            active_execution_provider: "cpu".to_string(),
            active_device: "cpu".to_string(),
            fallback_reason,
        }
    }

    #[cfg(feature = "openvino")]
    fn openvino(config: &InferenceConfig) -> Self {
        Self {
            requested_execution_provider: config.execution_provider.clone(),
            requested_device: config.device.clone(),
            active_execution_provider: "openvino".to_string(),
            active_device: config.device.clone(),
            fallback_reason: None,
        }
    }
}

pub fn runtime_version() -> anyhow::Result<String> {
    let base = unsafe { ort::sys::OrtGetApiBase() };
    if base.is_null() {
        anyhow::bail!("the ONNX Runtime library returned a null API base pointer");
    }
    let version = unsafe { CStr::from_ptr(((*base).GetVersionString)()) };
    Ok(version.to_string_lossy().into_owned())
}

pub fn ensure_supported_runtime() -> anyhow::Result<()> {
    let version = runtime_version()?;
    let base = unsafe { ort::sys::OrtGetApiBase() };
    let api = unsafe { ((*base).GetApi)(ort::sys::ORT_API_VERSION) };
    if api.is_null() {
        anyhow::bail!(
            "the ONNX Runtime library loaded at startup is version {version}, \
             which is older than the 1.{}.x this build of Gaze requires; \
             install a newer ONNX Runtime, or rebuild Gaze against the one you have",
            ort::MINOR_VERSION
        );
    }
    Ok(())
}

fn build_cpu_session(model_path: &str) -> anyhow::Result<Session> {
    Session::builder()
        .map_err(|error| {
            anyhow::anyhow!("failed to create an ONNX Runtime session builder: {error}")
        })?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|error| {
            anyhow::anyhow!("failed to set the ONNX Runtime graph optimization level: {error}")
        })?
        .commit_from_file(model_path)
        .with_context(|| format!("failed to load ONNX model {model_path} on cpu"))
}

#[cfg(feature = "openvino")]
fn build_openvino_session(model_path: &str, device: &str) -> anyhow::Result<Session> {
    let openvino_device = device.to_ascii_uppercase();
    Session::builder()
        .map_err(|error| {
            anyhow::anyhow!("failed to create an ONNX Runtime session builder: {error}")
        })?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|error| {
            anyhow::anyhow!("failed to set the ONNX Runtime graph optimization level: {error}")
        })?
        .with_execution_providers([ep::OpenVINO::default()
            .with_device_type(&openvino_device)
            .build()
            .error_on_failure()])
        .map_err(|error| {
            anyhow::anyhow!(
                "failed to register the OpenVINO Execution Provider for {device}: {error}"
            )
        })?
        .commit_from_file(model_path)
        .with_context(|| format!("failed to load ONNX model {model_path} on {device}"))
}

fn unusable_reason(config: &InferenceConfig) -> Option<String> {
    #[cfg(not(feature = "openvino"))]
    if config.execution_provider == "openvino" {
        return Some(
            "this Gaze build does not include OpenVINO support; rebuild with the \"openvino\" Cargo feature"
                .to_string(),
        );
    }

    config.validate().err().map(|error| error.to_string())
}

pub fn create_session(
    model_path: &str,
    config: &InferenceConfig,
) -> anyhow::Result<(Session, InferenceRuntime)> {
    if let Some(reason) = unusable_reason(config) {
        tracing::warn!(
            reason,
            "Unusable inference configuration; falling back to the ONNX Runtime CPU execution provider"
        );
        let session = build_cpu_session(model_path)?;
        return Ok((session, InferenceRuntime::cpu(config, Some(reason))));
    }

    #[cfg(feature = "openvino")]
    if config.execution_provider == "openvino" {
        match build_openvino_session(model_path, &config.device) {
            Ok(session) => {
                tracing::info!(
                    execution_provider = "openvino",
                    device = config.device,
                    model = model_path,
                    "Loaded ONNX model"
                );
                return Ok((session, InferenceRuntime::openvino(config)));
            }
            Err(error) => {
                let reason = error.to_string();
                tracing::warn!(
                    execution_provider = "openvino",
                    device = config.device,
                    model = model_path,
                    reason,
                    "OpenVINO model setup failed; falling back to the ONNX Runtime CPU execution provider"
                );
                let session = build_cpu_session(model_path).with_context(|| {
                    format!(
                        "OpenVINO setup failed ({reason}) and the ONNX Runtime CPU fallback also failed"
                    )
                })?;
                return Ok((session, InferenceRuntime::cpu(config, Some(reason))));
            }
        }
    }

    let session = build_cpu_session(model_path)?;
    tracing::info!(
        execution_provider = "cpu",
        device = "cpu",
        model = model_path,
        "Loaded ONNX model"
    );
    Ok((session, InferenceRuntime::cpu(config, None)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(feature = "openvino")]
    fn runtime_names_remain_lowercase() {
        let config = InferenceConfig {
            execution_provider: "openvino".to_string(),
            device: "gpu".to_string(),
        };
        let runtime = InferenceRuntime::openvino(&config);
        assert_eq!(runtime.active_execution_provider, "openvino");
        assert_eq!(runtime.active_device, "gpu");
    }

    #[test]
    #[cfg(not(feature = "openvino"))]
    fn cpu_only_build_falls_back_for_openvino_instead_of_refusing_to_start() {
        let config = InferenceConfig {
            execution_provider: "openvino".to_string(),
            device: "gpu".to_string(),
        };
        let reason = unusable_reason(&config).expect("a cpu-only build cannot honour openvino");
        assert!(reason.contains("does not include OpenVINO support"));
    }

    #[test]
    fn the_linked_onnx_runtime_supports_the_api_version_gaze_asks_for() {
        let version = runtime_version().expect("the linked ONNX Runtime reports a version");
        ensure_supported_runtime().unwrap_or_else(|error| {
            panic!(
                "ort asks for API {}, linked runtime {version}: {error}",
                ort::MINOR_VERSION
            )
        });
    }

    #[test]
    fn an_unsupported_api_version_is_detectable_without_aborting() {
        let base = unsafe { ort::sys::OrtGetApiBase() };
        assert!(!base.is_null());
        assert!(unsafe { ((*base).GetApi)(u32::MAX) }.is_null());
    }

    #[test]
    fn a_usable_cpu_configuration_has_no_fallback_reason() {
        assert_eq!(unusable_reason(&InferenceConfig::default()), None);
    }

    #[test]
    fn an_invalid_device_falls_back_rather_than_failing() {
        let config = InferenceConfig {
            execution_provider: "cpu".to_string(),
            device: "npu".to_string(),
        };
        let reason = unusable_reason(&config).expect("cpu cannot drive an npu");
        assert!(reason.contains("inference.device"));
    }
}
