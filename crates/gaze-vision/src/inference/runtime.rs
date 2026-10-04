// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use anyhow::Context;
use gaze_core::{acceleration, config::InferenceConfig};
use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Debug)]
pub struct RuntimeLibrary {
    pub path: PathBuf,
    pub version: String,
    pub provider: String,
    pub fallback_reason: Option<String>,
}

static RUNTIME: OnceLock<RuntimeLibrary> = OnceLock::new();
static INITIALIZE: Mutex<()> = Mutex::new(());

fn requested_provider<'a>(
    config: &'a InferenceConfig,
    devices: &[acceleration::NpuDevice],
) -> &'a str {
    if config.execution_provider == "auto" {
        devices.first().map_or("cpu", |device| device.provider)
    } else {
        &config.execution_provider
    }
}

fn cpu_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = std::env::var_os("GAZE_CPU_ORT_PATH") {
        paths.push(path.into());
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        paths.push(dir.join("libonnxruntime.so"));
        // Cargo test executables live in target/release/deps.
        if dir.file_name().is_some_and(|name| name == "deps")
            && let Some(parent) = dir.parent()
        {
            paths.push(parent.join("libonnxruntime.so"));
        }
    }
    paths.extend([
        PathBuf::from("/usr/lib/gaze/libonnxruntime.so"),
        PathBuf::from("/usr/lib64/gaze/libonnxruntime.so"),
    ]);
    paths.push(PathBuf::from("libonnxruntime.so"));
    paths
}

/// Vendor SDKs need their own dependency search path. Re-exec before Tokio starts any
/// threads; changing LD_LIBRARY_PATH in a running process does not update the ELF loader.
pub fn prepare_environment(config: &InferenceConfig) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    if config.validate().is_err() || std::env::var_os("ORT_DYLIB_PATH").is_some() {
        return Ok(());
    }
    let devices = acceleration::discover_npus();
    let provider = requested_provider(config, &devices);
    if provider == "cpu" {
        return Ok(());
    }
    match std::env::var("GAZE_RUNTIME_ENV").ok().as_deref() {
        None => {}
        Some(env) if env == provider => {
            // The CPU fallback must not resolve its dependencies from the vendor's
            // directories, so drop them while this process is still single-threaded.
            let Err(error) = library_version(&acceleration::vendor_runtime(provider)) else {
                return Ok(());
            };
            tracing::warn!(provider, error = %format!("{error:#}"), "Vendor runtime unusable; restarting without its library path");
            let mut command = std::process::Command::new(std::env::current_exe()?);
            command
                .args(std::env::args_os().skip(1))
                .env("GAZE_RUNTIME_ENV", "cpu")
                .env_remove("GAZE_SYSTEM_LD_LIBRARY_PATH");
            match std::env::var_os("GAZE_SYSTEM_LD_LIBRARY_PATH") {
                Some(original) => command.env("LD_LIBRARY_PATH", original),
                None => command.env_remove("LD_LIBRARY_PATH"),
            };
            let error = command.exec();
            return Err(error)
                .context("failed to restart gazed without the vendor SDK library path");
        }
        Some(_) => return Ok(()),
    }
    let file = Path::new(acceleration::RUNTIME_DIR)
        .join(provider)
        .join("library-path");
    let Ok(paths) = std::fs::read_to_string(&file) else {
        return Ok(());
    };
    let paths = paths.trim();
    if paths.is_empty() || !paths.split(':').all(|p| Path::new(p).is_absolute()) {
        tracing::warn!(path = %file.display(), "Invalid vendor library directories; trying the runtime without them");
        return Ok(());
    }
    let mut search = paths.to_string();
    let mut command = std::process::Command::new(std::env::current_exe()?);
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        search.push(':');
        search.push_str(&existing.to_string_lossy());
        command.env("GAZE_SYSTEM_LD_LIBRARY_PATH", existing);
    }
    let error = command
        .args(std::env::args_os().skip(1))
        .env("LD_LIBRARY_PATH", search)
        .env("GAZE_RUNTIME_ENV", provider)
        .exec();
    Err(error).context("failed to restart gazed with the vendor SDK library path")
}

/// Validate the actual API, not only the version label, before ort's global initialization.
/// Failure here leaves it possible to try another library rather than panicking in ort::api().
fn library_version(path: &Path) -> anyhow::Result<String> {
    let filename = CString::new(path.as_os_str().as_bytes())?;
    struct Handle(*mut libc::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { libc::dlclose(self.0) };
        }
    }
    // ONNX Runtime and vendor plugins are not safe to unload, and ort reopens this same
    // library right after the check; NODELETE keeps it resident across the dlclose.
    let handle = unsafe {
        libc::dlopen(
            filename.as_ptr(),
            libc::RTLD_NOW | libc::RTLD_LOCAL | libc::RTLD_NODELETE,
        )
    };
    if handle.is_null() {
        let error = unsafe { libc::dlerror() };
        let reason = if error.is_null() {
            "unknown dynamic loader error".to_string()
        } else {
            unsafe { CStr::from_ptr(error) }
                .to_string_lossy()
                .into_owned()
        };
        anyhow::bail!("cannot load {}: {reason}", path.display());
    }
    let handle = Handle(handle);
    let symbol = unsafe { libc::dlsym(handle.0, c"OrtGetApiBase".as_ptr()) };
    if symbol.is_null() {
        anyhow::bail!("{} does not export OrtGetApiBase", path.display());
    }
    let get_base: unsafe extern "system" fn() -> *const ort::sys::OrtApiBase =
        unsafe { std::mem::transmute(symbol) };
    let base = unsafe { get_base() };
    if base.is_null() {
        anyhow::bail!("{} returned a null ONNX Runtime API base", path.display());
    }
    let version_ptr = unsafe { ((*base).GetVersionString)() };
    if version_ptr.is_null() {
        anyhow::bail!("{} returned a null version string", path.display());
    }
    let version = unsafe { CStr::from_ptr(version_ptr) }
        .to_string_lossy()
        .into_owned();
    if unsafe { ((*base).GetApi)(ort::sys::ORT_API_VERSION) }.is_null() {
        anyhow::bail!(
            "ONNX Runtime {version} at {} does not support API {}; Gaze requires 1.{}.x or newer",
            path.display(),
            ort::sys::ORT_API_VERSION,
            ort::MINOR_VERSION
        );
    }
    Ok(version)
}

pub fn initialize_runtime(config: &InferenceConfig) -> anyhow::Result<&'static RuntimeLibrary> {
    let _guard = INITIALIZE.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime);
    }
    let devices = acceleration::discover_npus();
    let provider = if config.validate().is_ok() {
        requested_provider(config, &devices)
    } else {
        "cpu"
    };
    let mut candidates = Vec::new();
    // An explicit administrator override also supports distro/Nix-managed SDKs.
    if let Some(path) = std::env::var_os("ORT_DYLIB_PATH") {
        candidates.push((provider.to_string(), PathBuf::from(path)));
    } else if provider != "cpu" {
        candidates.push((provider.to_string(), acceleration::vendor_runtime(provider)));
    }
    candidates.extend(
        cpu_candidates()
            .into_iter()
            .map(|path| ("cpu".to_string(), path)),
    );
    let mut errors = Vec::new();
    for (active_provider, path) in candidates {
        // Resolve registered SDK symlinks so $ORIGIN dependencies keep their original layout.
        let path = path.canonicalize().unwrap_or(path);
        let attempt = (|| {
            let version = library_version(&path)?;
            let initialized = ort::init_from(&path)?
                .with_name("gazed")
                .with_logger(std::sync::Arc::new(
                    |level, category, _id, _location, message| {
                        tracing::debug!(target: "ort", "[{category}] ({level:?}) {message}");
                    },
                ))
                .commit();
            anyhow::ensure!(
                initialized,
                "ONNX Runtime was initialized before Gaze selected a runtime"
            );
            Ok::<_, anyhow::Error>(version)
        })();
        match attempt {
            Ok(version) => {
                let fallback_reason = if active_provider == "cpu" && provider != "cpu" {
                    Some(format!(
                        "{provider} runtime unavailable: {}. Register it under {} and restart gazed",
                        errors.join("; "),
                        acceleration::RUNTIME_DIR
                    ))
                } else if config.execution_provider == "auto" && devices.is_empty() {
                    Some("no supported NPU driver found in /sys/class/accel; using CPU".to_string())
                } else {
                    None
                };
                tracing::info!(path = %path.display(), %version, provider = active_provider, "Loaded ONNX Runtime");
                let runtime = RuntimeLibrary {
                    path,
                    version,
                    provider: active_provider,
                    fallback_reason,
                };
                RUNTIME
                    .set(runtime)
                    .expect("runtime initialization is serialized");
                return Ok(RUNTIME.get().unwrap());
            }
            Err(error) => errors.push(format!("{error:#}")),
        }
    }
    Err(anyhow::anyhow!(
        "no usable ONNX Runtime: {}",
        errors.join("; ")
    ))
    .context(
        "install the Gaze runtime package, or set GAZE_CPU_ORT_PATH to a compatible CPU library",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_uses_an_npu_driver_and_explicit_choices_are_preserved() {
        let device = acceleration::NpuDevice {
            node: "/dev/accel/accel0".into(),
            driver: "amdxdna".into(),
            provider: "vitis",
        };
        let auto = InferenceConfig {
            execution_provider: "auto".into(),
            device: "npu".into(),
        };
        assert_eq!(requested_provider(&auto, &[]), "cpu");
        assert_eq!(requested_provider(&auto, &[device]), "vitis");
        let explicit = InferenceConfig {
            execution_provider: "openvino".into(),
            device: "npu".into(),
        };
        assert_eq!(requested_provider(&explicit, &[]), "openvino");
    }

    #[test]
    fn missing_runtime_returns_an_actionable_error_without_panicking() {
        let error = library_version(Path::new("/nonexistent/gaze/libonnxruntime.so")).unwrap_err();
        assert!(error.to_string().contains("cannot load"));
    }

    #[test]
    fn packaged_cpu_runtime_exposes_the_required_api() {
        let runtime = initialize_runtime(&InferenceConfig::default()).unwrap();
        assert!(!runtime.version.is_empty());
        assert_eq!(library_version(&runtime.path).unwrap(), runtime.version);
    }
}
