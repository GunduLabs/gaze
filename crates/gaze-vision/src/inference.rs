// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::Context;
use gaze_core::config::InferenceConfig;
use ort::ep::{self, ArbitrarilyConfigurableExecutionProvider, ExecutionProvider};
use ort::session::{
    Session,
    builder::{GraphOptimizationLevel, SessionBuilder},
};
use runtime::RuntimeLibrary;
use sha2::{Digest, Sha256};

mod runtime;

pub use runtime::{initialize_runtime, prepare_environment};

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

    fn accelerated(config: &InferenceConfig, provider: &str) -> Self {
        Self {
            requested_execution_provider: config.execution_provider.clone(),
            requested_device: config.device.clone(),
            active_execution_provider: provider.to_string(),
            active_device: config.device.clone(),
            fallback_reason: None,
        }
    }
}

fn session_builder() -> anyhow::Result<SessionBuilder> {
    Session::builder()
        .map_err(|error| {
            anyhow::anyhow!("failed to create an ONNX Runtime session builder: {error}")
        })?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(|error| {
            anyhow::anyhow!("failed to set the ONNX Runtime graph optimization level: {error}")
        })
}

fn build_cpu_session(model_path: &str) -> anyhow::Result<Session> {
    session_builder()?
        .commit_from_file(model_path)
        .with_context(|| format!("failed to load ONNX model {model_path} on cpu"))
}

fn cache_dir(
    library: &RuntimeLibrary,
    model_path: &str,
    provider: &str,
) -> anyhow::Result<std::path::PathBuf> {
    let model_hash = Sha256::digest(std::fs::read(model_path)?);
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let namespace = Sha256::digest(format!(
        "{}:{}:{}",
        library.path.display(),
        library.version,
        kernel
    ));
    let root = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/var/cache/gaze"));
    let dir = root
        .join("inference")
        .join(provider)
        .join(
            namespace
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        )
        .join(
            model_hash
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        );
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create inference cache {}", dir.display()))?;
    Ok(dir)
}

fn build_accelerated_session(
    library: &RuntimeLibrary,
    model_path: &str,
    provider: &str,
    device: &str,
) -> anyhow::Result<Session> {
    let available = match provider {
        "openvino" => ep::OpenVINO::default().is_available()?,
        "vitis" => ep::Vitis::default().is_available()?,
        _ => false,
    };
    anyhow::ensure!(
        available,
        "loaded ONNX Runtime does not include the {provider} execution provider; register its runtime and restart gazed"
    );
    let cache = cache_dir(library, model_path, provider)?;
    let cache = cache.to_string_lossy();
    let dispatch = match provider {
        "openvino" => {
            let ep = ep::OpenVINO::default()
                .with_device_type(device.to_ascii_uppercase())
                .with_cache_dir(&cache);
            ep.build().error_on_failure()
        }
        "vitis" => {
            let ep = ep::Vitis::default()
                .with_cache_dir(&cache)
                .with_arbitrary_config("enable_cache_file_io_in_mem", "0");
            ep.build().error_on_failure()
        }
        _ => anyhow::bail!("unsupported acceleration provider {provider}"),
    };
    let mut builder = session_builder()?;
    if device == "npu"
        && let Some(size) = model_input_size(model_path)
    {
        // The SCRFD models have symbolic dimensions. Freeze the actual NCHW input
        // Gaze supplies so both vendors can compile a static graph for their NPU.
        let metadata = build_cpu_session(model_path)?;
        if let Some(input) = metadata.inputs().first()
            && let ort::value::ValueType::Tensor {
                shape,
                dimension_symbols,
                ..
            } = input.dtype()
            && shape.len() == 4
        {
            for (index, symbol) in dimension_symbols.iter().enumerate() {
                if shape[index] < 0 && !symbol.is_empty() {
                    builder = builder
                        .with_dimension_override(symbol, [1, 3, size as i64, size as i64][index])
                        .map_err(|error| {
                            anyhow::anyhow!("failed to freeze input dimensions: {error}")
                        })?;
                }
            }
        }
    }
    let mut session = builder
        .with_execution_providers([dispatch])
        .map_err(|error| anyhow::anyhow!("failed to register {provider}: {error}"))?
        .commit_from_file(model_path)
        .with_context(|| {
            format!("failed to load ONNX model {model_path} on {provider}/{device}")
        })?;
    // Catch lazy compilation/shape/driver errors before the first authentication attempt.
    probe_session(&mut session, model_path)?;
    Ok(session)
}

fn model_input_size(model_path: &str) -> Option<usize> {
    let name = std::path::Path::new(model_path)
        .file_name()
        .and_then(|s| s.to_str());
    match name {
        Some("det_500m.onnx" | "det_10g.onnx") => Some(320),
        Some("w600k_mbf.onnx" | "w600k_r50.onnx") => Some(112),
        Some("minifasnet_v2.onnx") => Some(80),
        Some("open_closed_eye.onnx") => Some(32),
        _ => None,
    }
}

fn probe_session(session: &mut Session, model_path: &str) -> anyhow::Result<()> {
    let Some(size) = model_input_size(model_path) else {
        return Ok(());
    };
    let input = ndarray::Array4::<f32>::zeros((1, 3, size, size));
    session
        .run(ort::inputs![ort::value::TensorRef::from_array_view(
            &input
        )?])
        .with_context(|| format!("{model_path} failed its accelerator warmup"))?;
    Ok(())
}

fn unusable_reason(config: &InferenceConfig) -> Option<String> {
    config.validate().err().map(|error| error.to_string())
}

pub fn create_session(
    model_path: &str,
    config: &InferenceConfig,
) -> anyhow::Result<(Session, InferenceRuntime)> {
    let library = initialize_runtime(config)?;
    if let Some(reason) = unusable_reason(config) {
        tracing::warn!(
            reason,
            "Unusable inference configuration; falling back to the ONNX Runtime CPU execution provider"
        );
        let session = build_cpu_session(model_path)?;
        return Ok((session, InferenceRuntime::cpu(config, Some(reason))));
    }
    if config.execution_provider == "cpu" {
        return Ok((
            build_cpu_session(model_path)?,
            InferenceRuntime::cpu(config, None),
        ));
    }
    let provider = if config.execution_provider == "auto" {
        library.provider.as_str()
    } else {
        config.execution_provider.as_str()
    };
    let reason = if provider == "cpu" {
        library.fallback_reason.clone().unwrap_or_else(|| {
            "no accelerator runtime selected; restart gazed after installing a vendor runtime"
                .to_string()
        })
    } else if library.provider != provider && library.provider != "cpu" {
        format!(
            "the loaded runtime is {}; restart gazed to select {provider}",
            library.provider
        )
    } else {
        match build_accelerated_session(library, model_path, provider, &config.device) {
            Ok(session) => {
                tracing::info!(
                    provider,
                    device = config.device,
                    model = model_path,
                    "Loaded accelerated ONNX session (CPU graph partitions may remain)"
                );
                return Ok((session, InferenceRuntime::accelerated(config, provider)));
            }
            Err(error) => match &library.fallback_reason {
                Some(detail) => format!("{error:#}. {detail}"),
                None => format!("{error:#}"),
            },
        }
    };
    tracing::warn!(provider, model = model_path, %reason, "Accelerator setup failed; using CPU");
    let session = build_cpu_session(model_path).with_context(|| {
        format!("accelerator setup failed ({reason}) and CPU fallback also failed")
    })?;
    Ok((session, InferenceRuntime::cpu(config, Some(reason))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_names_remain_lowercase() {
        let config = InferenceConfig {
            execution_provider: "vitis".into(),
            device: "npu".into(),
        };
        let runtime = InferenceRuntime::accelerated(&config, "vitis");
        assert_eq!(runtime.active_execution_provider, "vitis");
        assert_eq!(runtime.active_device, "npu");
    }

    #[test]
    fn usable_configurations_do_not_depend_on_cargo_features() {
        for provider in ["auto", "openvino", "vitis"] {
            let config = InferenceConfig {
                execution_provider: provider.into(),
                device: "npu".into(),
            };
            assert_eq!(unusable_reason(&config), None);
        }
        assert_eq!(unusable_reason(&InferenceConfig::default()), None);
    }

    #[test]
    fn an_invalid_device_falls_back_rather_than_failing() {
        let config = InferenceConfig {
            execution_provider: "cpu".into(),
            device: "npu".into(),
        };
        assert!(
            unusable_reason(&config)
                .unwrap()
                .contains("inference.device")
        );
    }

    // ONNX Identity (opset 13, float NCHW 1x3x2x2); no trained model weights.
    const IDENTITY_MODEL: &[u8] = &[
        8, 8, 18, 4, 103, 97, 122, 101, 58, 87, 10, 16, 10, 1, 120, 18, 1, 121, 34, 8, 73, 100,
        101, 110, 116, 105, 116, 121, 18, 9, 103, 97, 122, 101, 45, 116, 101, 115, 116, 90, 27, 10,
        1, 120, 18, 22, 10, 20, 8, 1, 18, 16, 10, 2, 8, 1, 10, 2, 8, 3, 10, 2, 8, 2, 10, 2, 8, 2,
        98, 27, 10, 1, 121, 18, 22, 10, 20, 8, 1, 18, 16, 10, 2, 8, 1, 10, 2, 8, 3, 10, 2, 8, 2,
        10, 2, 8, 2, 66, 2, 16, 13,
    ];

    #[test]
    fn missing_vendor_providers_keep_cpu_inference_working() {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "gaze-identity-{}-{}.onnx",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, IDENTITY_MODEL).unwrap();
        let input = ndarray::Array4::<f32>::from_elem((1, 3, 2, 2), 0.75);
        for provider in ["cpu", "auto", "openvino", "vitis"] {
            let config = InferenceConfig {
                execution_provider: provider.into(),
                device: if provider == "cpu" { "cpu" } else { "npu" }.into(),
            };
            let (mut session, runtime) = create_session(path.to_str().unwrap(), &config).unwrap();
            let outputs = session
                .run(ort::inputs![
                    ort::value::TensorRef::from_array_view(&input).unwrap()
                ])
                .unwrap();
            let (_, data) = outputs[0].try_extract_tensor::<f32>().unwrap();
            assert!(data.iter().all(|value| *value == 0.75));
            if runtime.active_execution_provider == "cpu" && provider != "cpu" {
                assert!(runtime.fallback_reason.is_some());
            }
        }
        std::fs::remove_file(path).unwrap();
    }

    // Fallback hides a broken provider, so this asserts the session really used OpenVINO.
    #[test]
    #[ignore = "needs Intel's OpenVINO ONNX Runtime; run `just test-openvino`"]
    fn openvino_runtime_creates_an_openvino_session() {
        let path = std::env::temp_dir().join(format!(
            "gaze-identity-openvino-{}.onnx",
            std::process::id()
        ));
        std::fs::write(&path, IDENTITY_MODEL).unwrap();
        let config = InferenceConfig {
            execution_provider: "openvino".into(),
            device: "cpu".into(),
        };
        let (_, runtime) = create_session(path.to_str().unwrap(), &config).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            runtime.active_execution_provider, "openvino",
            "OpenVINO fell back to CPU: {:?}",
            runtime.fallback_reason
        );
    }
}
