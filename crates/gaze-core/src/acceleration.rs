// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shared NPU discovery for the daemon and diagnostics, without loading ML libraries.

use std::path::{Path, PathBuf};

pub const RUNTIME_DIR: &str = "/usr/lib/gaze/runtimes";

/// Distro-packaged ONNX Runtime builds that can serve a vendor provider without registration.
pub const SYSTEM_RUNTIMES: &[&str] = &[
    "/usr/lib64/libonnxruntime.so.1",
    "/usr/lib/libonnxruntime.so.1",
    "/usr/lib/x86_64-linux-gnu/libonnxruntime.so.1",
    "/usr/lib/aarch64-linux-gnu/libonnxruntime.so.1",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NpuDevice {
    pub node: PathBuf,
    pub driver: String,
    pub provider: &'static str,
}

pub fn provider_for_driver(driver: &str) -> Option<&'static str> {
    match driver {
        "intel_vpu" => Some("openvino"),
        "amdxdna" => Some("vitis"),
        _ => None,
    }
}

pub fn discover_npus() -> Vec<NpuDevice> {
    discover_npus_at(Path::new("/sys/class/accel"), Path::new("/dev/accel"))
}

fn discover_npus_at(sysfs: &Path, nodes: &Path) -> Vec<NpuDevice> {
    let Ok(entries) = std::fs::read_dir(sysfs) else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(index) = name.to_str().and_then(|name| name.strip_prefix("accel")) else {
            continue;
        };
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(driver_path) = std::fs::read_link(entry.path().join("device/driver")) else {
            continue;
        };
        let Some(driver) = driver_path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if let Some(provider) = provider_for_driver(driver) {
            devices.push(NpuDevice {
                node: nodes.join(name),
                driver: driver.to_string(),
                provider,
            });
        }
    }
    devices.sort_by(|a, b| a.node.cmp(&b.node));
    devices
}

/// The registered vendor runtime, else a system one, else the path to register.
pub fn provider_runtime(provider: &str) -> PathBuf {
    let vendor = Path::new(RUNTIME_DIR)
        .join(provider)
        .join("libonnxruntime.so");
    std::iter::once(vendor.clone())
        .chain(SYSTEM_RUNTIMES.iter().map(PathBuf::from))
        .find(|path| path.is_file())
        .unwrap_or(vendor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn matches_npu_drivers_instead_of_cpu_or_gpu_vendor_names() {
        assert_eq!(provider_for_driver("intel_vpu"), Some("openvino"));
        assert_eq!(provider_for_driver("amdxdna"), Some("vitis"));
        for driver in ["amdgpu", "i915", "xe", "", "unknown"] {
            assert_eq!(provider_for_driver(driver), None);
        }
    }

    #[test]
    fn discovers_both_vendors_and_ignores_unbound_or_unrelated_devices() {
        let root = std::env::temp_dir().join(format!("gaze-npu-discovery-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for (name, driver) in [
            ("accel0", "intel_vpu"),
            ("accel1", "amdxdna"),
            ("accel2", "unknown"),
            ("accel_bad", "amdxdna"),
        ] {
            let device = root.join(name).join("device");
            std::fs::create_dir_all(&device).unwrap();
            symlink(
                format!("/sys/bus/pci/drivers/{driver}"),
                device.join("driver"),
            )
            .unwrap();
        }
        std::fs::create_dir_all(root.join("accel3/device")).unwrap();
        let devices = discover_npus_at(&root, Path::new("/dev/accel"));
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].provider, "openvino");
        assert_eq!(devices[1].provider, "vitis");
        assert_eq!(devices[1].node, Path::new("/dev/accel/accel1"));
    }

    #[test]
    fn absent_accel_class_is_a_cpu_only_machine() {
        assert!(
            discover_npus_at(Path::new("/nonexistent/gaze-accel"), Path::new("/dev")).is_empty()
        );
    }
}
