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
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

mod runtime;

pub use runtime::{initialize_runtime, prepare_environment};

/// A regular file and its digest, verified during model acquisition.
/// Session caches reuse the digest instead of hashing the weights again.
#[derive(Clone, Debug)]
pub struct ModelFile {
    path: PathBuf,
    sha256: String,
}

impl ModelFile {
    pub fn verify(path: impl Into<PathBuf>, expected: Option<&str>) -> anyhow::Result<Self> {
        let path = path.into();
        let metadata = std::fs::symlink_metadata(&path)?;
        anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "model path is not a regular file: {}",
            path.display()
        );
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        let sha256 = hasher
            .finalize()
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                use std::fmt::Write;
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        if let Some(expected) = expected {
            anyhow::ensure!(
                sha256 == expected,
                "checksum mismatch for {}: expected {expected}, got {sha256}",
                path.display()
            );
        }
        Ok(Self { path, sha256 })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    /// Publish verified bytes without copying or hashing them again.
    pub fn rename(mut self, destination: &Path) -> anyhow::Result<Self> {
        std::fs::rename(&self.path, destination)?;
        self.path = destination.to_path_buf();
        Ok(self)
    }
}

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

fn session_builder(library: &RuntimeLibrary) -> anyhow::Result<SessionBuilder> {
    let configure_error =
        |error| anyhow::anyhow!("failed to configure ONNX Runtime session: {error}");
    Session::builder(&library.environment)
        .map_err(|error| {
            anyhow::anyhow!("failed to create an ONNX Runtime session builder: {error}")
        })?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(configure_error)?
        .with_intra_threads(inference_threads(
            std::thread::available_parallelism().map_or(1, usize::from),
        ))
        .map_err(configure_error)?
        .with_inter_threads(1)
        .map_err(configure_error)?
        .with_parallel_execution(false)
        .map_err(configure_error)?
        .with_intra_op_spinning(false)
        .map_err(configure_error)?
        .with_inter_op_spinning(false)
        .map_err(configure_error)
}

fn inference_threads(available: usize) -> usize {
    available.clamp(1, 2)
}

fn build_cpu_session(library: &RuntimeLibrary, model: &ModelFile) -> anyhow::Result<Session> {
    session_builder(library)?
        .commit_from_file(model.path())
        .with_context(|| {
            format!(
                "failed to load ONNX model {} on cpu",
                model.path().display()
            )
        })
}

fn cache_dir(
    library: &RuntimeLibrary,
    model: &ModelFile,
    provider: &str,
) -> anyhow::Result<std::path::PathBuf> {
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
        .join(model.sha256());
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("cannot create inference cache {}", dir.display()))?;
    Ok(dir)
}

fn build_accelerated_session(
    library: &RuntimeLibrary,
    model: &ModelFile,
    provider: &str,
    device: &str,
) -> anyhow::Result<Session> {
    let available = match provider {
        "openvino" => ep::OpenVINO::is_available()?,
        "vitis" => ep::Vitis::is_available()?,
        _ => false,
    };
    anyhow::ensure!(
        available,
        "loaded ONNX Runtime does not include the {provider} execution provider; register its runtime and restart gazed"
    );
    let cache = cache_dir(library, model, provider)?;
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
    let mut builder = session_builder(library)?;
    if device == "npu" {
        // Read only ONNX input metadata; no second session or model weights are needed.
        for (symbol, size) in model_dimension_overrides(model.path())? {
            builder = builder
                .with_dimension_override(symbol, size)
                .map_err(|error| anyhow::anyhow!("failed to freeze input dimensions: {error}"))?;
        }
    }
    let mut session = builder
        .with_execution_providers([dispatch])
        .map_err(|error| anyhow::anyhow!("failed to register {provider}: {error}"))?
        .commit_from_file(model.path())
        .with_context(|| {
            format!(
                "failed to load ONNX model {} on {provider}/{device}",
                model.path().display()
            )
        })?;
    // Catch lazy compilation/shape/driver errors before the first authentication attempt.
    probe_session(&mut session, model.path())?;
    Ok(session)
}

fn proto_varint(reader: &mut (impl Read + Seek), end: u64) -> anyhow::Result<u64> {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        anyhow::ensure!(
            reader.stream_position()? < end,
            "truncated ONNX protobuf value"
        );
        let mut byte = [0u8];
        reader.read_exact(&mut byte)?;
        anyhow::ensure!(
            shift != 63 || byte[0] <= 1,
            "overflowing ONNX protobuf value"
        );
        value |= u64::from(byte[0] & 0x7f) << shift;
        if byte[0] & 0x80 == 0 {
            return Ok(value);
        }
    }
    anyhow::bail!("invalid ONNX protobuf value")
}

/// Find a length-delimited field, seeking past weights without allocating them.
fn proto_field(
    reader: &mut (impl Read + Seek),
    region: std::ops::Range<u64>,
    wanted: u64,
) -> anyhow::Result<Option<std::ops::Range<u64>>> {
    reader.seek(SeekFrom::Start(region.start))?;
    while reader.stream_position()? < region.end {
        let key = proto_varint(reader, region.end)?;
        anyhow::ensure!(key >> 3 != 0, "invalid ONNX protobuf field");
        let length = match key & 7 {
            0 => {
                proto_varint(reader, region.end)?;
                continue;
            }
            1 => 8,
            2 => proto_varint(reader, region.end)?,
            5 => 4,
            _ => anyhow::bail!("unsupported ONNX protobuf wire type"),
        };
        let start = reader.stream_position()?;
        let end = start
            .checked_add(length)
            .context("ONNX protobuf length overflow")?;
        anyhow::ensure!(end <= region.end, "truncated ONNX protobuf field");
        if key >> 3 == wanted && key & 7 == 2 {
            return Ok(Some(start..end));
        }
        reader.seek(SeekFrom::Start(end))?;
    }
    Ok(None)
}

// Read only the graph's first input shape, seeking past initializers.
// NPU dimension overrides must not load an extra CPU session.
fn input_dimension_symbols(
    reader: &mut (impl Read + Seek),
    length: u64,
) -> anyhow::Result<Vec<Option<String>>> {
    let mut region = 0..length;
    // Model.graph -> Graph.input -> ValueInfo.type -> Type.tensor_type -> Tensor.shape.
    for field in [7, 11, 2, 1, 2] {
        let Some(next) = proto_field(reader, region, field)? else {
            return Ok(Vec::new());
        };
        region = next;
    }
    let mut symbols = Vec::new();
    while let Some(dimension) = proto_field(reader, region.clone(), 1)? {
        region.start = dimension.end;
        let symbol = match proto_field(reader, dimension, 2)? {
            Some(symbol) => {
                let length = symbol.end - symbol.start;
                anyhow::ensure!(length <= 1024, "ONNX dimension symbol is too long");
                let mut bytes = vec![0u8; length as usize];
                reader.seek(SeekFrom::Start(symbol.start))?;
                reader.read_exact(&mut bytes)?;
                Some(String::from_utf8(bytes).context("invalid ONNX dimension symbol")?)
            }
            None => None,
        };
        symbols.push(symbol);
        if symbols.len() > 4 {
            return Ok(Vec::new());
        }
    }
    Ok(symbols)
}

fn model_dimension_overrides(model_path: &Path) -> anyhow::Result<Vec<(String, i64)>> {
    let Some(size) = model_input_size(model_path) else {
        return Ok(Vec::new());
    };
    let mut file = std::fs::File::open(model_path)?;
    let length = file.metadata()?.len();
    let symbols = input_dimension_symbols(&mut file, length)?;
    nchw_overrides(symbols, size)
}

fn nchw_overrides(symbols: Vec<Option<String>>, size: usize) -> anyhow::Result<Vec<(String, i64)>> {
    if symbols.len() != 4 {
        return Ok(Vec::new());
    }
    let mut overrides = Vec::new();
    for (symbol, size) in symbols.into_iter().zip([1, 3, size as i64, size as i64]) {
        if let Some(symbol) = symbol.filter(|symbol| !symbol.is_empty()) {
            anyhow::ensure!(
                !overrides
                    .iter()
                    .any(|(name, value)| name == &symbol && *value != size),
                "conflicting ONNX dimension symbol {symbol}"
            );
            overrides.push((symbol, size));
        }
    }
    Ok(overrides)
}

fn model_input_size(model_path: &Path) -> Option<usize> {
    let name = model_path.file_name().and_then(|s| s.to_str());
    match name {
        Some("det_500m.onnx" | "det_10g.onnx") => Some(320),
        Some("w600k_mbf.onnx" | "w600k_r50.onnx") => Some(112),
        Some("minifasnet_v2.onnx") => Some(80),
        Some("open_closed_eye.onnx") => Some(32),
        _ => None,
    }
}

fn probe_session(session: &mut Session, model_path: &Path) -> anyhow::Result<()> {
    let Some(size) = model_input_size(model_path) else {
        return Ok(());
    };
    let input = ndarray::Array4::<f32>::zeros((1, 3, size, size));
    session
        .run(ort::inputs![ort::value::TensorRef::from_array_view(
            &input
        )?])
        .with_context(|| format!("{} failed its accelerator warmup", model_path.display()))?;
    Ok(())
}

fn unusable_reason(config: &InferenceConfig) -> Option<String> {
    config.validate().err().map(|error| error.to_string())
}

pub fn create_session(
    model: &ModelFile,
    config: &InferenceConfig,
) -> anyhow::Result<(Session, InferenceRuntime)> {
    let library = initialize_runtime(config)?;
    if let Some(reason) = unusable_reason(config) {
        tracing::warn!(
            reason,
            "Unusable inference configuration; falling back to the ONNX Runtime CPU execution provider"
        );
        let session = build_cpu_session(library, model)?;
        return Ok((session, InferenceRuntime::cpu(config, Some(reason))));
    }
    if config.execution_provider == "cpu" {
        return Ok((
            build_cpu_session(library, model)?,
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
        match build_accelerated_session(library, model, provider, &config.device) {
            Ok(session) => {
                tracing::info!(
                    provider,
                    device = config.device,
                    model = %model.path().display(),
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
    tracing::warn!(provider, model = %model.path().display(), %reason, "Accelerator setup failed; using CPU");
    let session = build_cpu_session(library, model).with_context(|| {
        format!("accelerator setup failed ({reason}) and CPU fallback also failed")
    })?;
    Ok((session, InferenceRuntime::cpu(config, Some(reason))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proto_message(field: u8, data: &[u8]) -> Vec<u8> {
        let mut bytes = vec![field << 3 | 2];
        let mut length = data.len();
        while length >= 128 {
            bytes.push((length as u8 & 0x7f) | 0x80);
            length >>= 7;
        }
        bytes.push(length as u8);
        bytes.extend_from_slice(data);
        bytes
    }

    fn shaped_model(symbols: &[Option<&str>]) -> Vec<u8> {
        let shape: Vec<u8> = symbols
            .iter()
            .flat_map(|symbol| {
                let dimension =
                    symbol.map_or_else(|| vec![8, 3], |symbol| proto_message(2, symbol.as_bytes()));
                proto_message(1, &dimension)
            })
            .collect();
        proto_message(
            7,
            &proto_message(
                11,
                &proto_message(2, &proto_message(1, &proto_message(2, &shape))),
            ),
        )
    }

    #[test]
    fn session_thread_counts_are_bounded_on_small_and_large_hosts() {
        assert_eq!(inference_threads(0), 1);
        assert_eq!(inference_threads(1), 1);
        for available in [2, 4, 16, 256] {
            assert_eq!(inference_threads(available), 2);
        }
    }

    #[test]
    fn onnx_input_metadata_is_read_without_creating_a_session() {
        let bytes = shaped_model(&[Some("batch"), None, Some("height"), Some("width")]);
        let symbols =
            input_dimension_symbols(&mut std::io::Cursor::new(&bytes), bytes.len() as u64).unwrap();
        assert_eq!(
            symbols,
            vec![
                Some("batch".into()),
                None,
                Some("height".into()),
                Some("width".into())
            ]
        );
        let symbols = input_dimension_symbols(
            &mut std::io::Cursor::new(IDENTITY_MODEL),
            IDENTITY_MODEL.len() as u64,
        )
        .unwrap();
        assert_eq!(symbols, vec![None, None, None, None]);
    }

    #[test]
    fn dynamic_dimensions_match_the_actual_nchw_input() {
        let symbols = vec![
            Some("batch".into()),
            Some("channels".into()),
            Some("height".into()),
            Some("width".into()),
        ];
        assert_eq!(
            nchw_overrides(symbols, 320).unwrap(),
            vec![
                ("batch".into(), 1),
                ("channels".into(), 3),
                ("height".into(), 320),
                ("width".into(), 320),
            ]
        );
        assert!(nchw_overrides(vec![Some("same".into()); 4], 320).is_err());
        assert!(nchw_overrides(vec![None; 3], 112).unwrap().is_empty());
    }

    #[test]
    fn malformed_or_oversized_onnx_metadata_is_rejected() {
        let oversized = "x".repeat(1025);
        let cases = [
            vec![0],
            vec![58, 127],
            vec![59],
            vec![255; 10],
            shaped_model(&[Some(&oversized)]),
        ];
        for bytes in cases {
            assert!(
                input_dimension_symbols(&mut std::io::Cursor::new(&bytes), bytes.len() as u64)
                    .is_err()
            );
        }
    }

    #[test]
    fn unknown_shapes_do_not_receive_nchw_overrides() {
        let bytes = shaped_model(&[None; 5]);
        assert!(
            input_dimension_symbols(&mut std::io::Cursor::new(&bytes), bytes.len() as u64)
                .unwrap()
                .is_empty()
        );
        assert!(
            model_dimension_overrides(Path::new("/nonexistent/custom.onnx"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn protobuf_scanning_seeks_past_weight_payloads() {
        struct MetadataOnly(std::io::Cursor<Vec<u8>>);
        impl Read for MetadataOnly {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                assert!(
                    !(3..1003).contains(&self.0.position()),
                    "weight data was read"
                );
                self.0.read(buf)
            }
        }
        impl Seek for MetadataOnly {
            fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
                self.0.seek(position)
            }
        }
        let mut bytes = proto_message(1, &vec![0; 1000]);
        bytes.extend(shaped_model(&[None; 4]));
        let length = bytes.len() as u64;
        assert_eq!(
            input_dimension_symbols(&mut MetadataOnly(std::io::Cursor::new(bytes)), length)
                .unwrap(),
            vec![None; 4]
        );
    }

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
        let model = ModelFile::verify(&path, None).unwrap();
        let input = ndarray::Array4::<f32>::from_elem((1, 3, 2, 2), 0.75);
        for provider in ["cpu", "auto", "openvino", "vitis"] {
            let config = InferenceConfig {
                execution_provider: provider.into(),
                device: if provider == "cpu" { "cpu" } else { "npu" }.into(),
            };
            let (mut session, runtime) = create_session(&model, &config).unwrap();
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
        let model = ModelFile::verify(&path, None).unwrap();
        let (_, runtime) = create_session(&model, &config).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            runtime.active_execution_provider, "openvino",
            "OpenVINO fell back to CPU: {:?}",
            runtime.fallback_reason
        );
    }
}
