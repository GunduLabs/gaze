// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use gaze_core::acceleration::{NpuDevice, discover_npus, vendor_runtime};
use std::path::Path;

fn register_fix(library: &Path) -> String {
    format!(
        "Link the vendor's libonnxruntime.so to {} and list its SDK library directories in {}; then restart gazed.",
        library.display(),
        library.with_file_name("library-path").display()
    )
}

// OpenVINO can also drive an Intel GPU or CPU, so only NPU devices need /sys/class/accel.
fn active_provider<'a>(config: Option<&'a Config>, devices: &[NpuDevice]) -> Option<&'a str> {
    let inference = &config?.inference;
    match inference.execution_provider.as_str() {
        "cpu" => None,
        "auto" => devices.first().map(|device| device.provider),
        provider => Some(provider),
    }
}

pub(super) fn check_acceleration(report: &mut Report, config: Option<&Config>) {
    let devices = discover_npus();
    let wants_npu = config.is_some_and(|config| {
        config.inference.execution_provider != "cpu" && config.inference.device == "npu"
    });
    if devices.is_empty() && wants_npu {
        report.warning(
            "NPU hardware",
            "no supported NPU driver is bound in /sys/class/accel",
            "Install your vendor's NPU kernel driver and firmware; an Intel or AMD CPU alone does not imply an NPU.",
        );
    }
    for device in &devices {
        report.pass(
            "NPU hardware",
            format!(
                "{} uses {} ({})",
                device.node.display(),
                device.driver,
                device.provider
            ),
        );
        if !device.node.exists() {
            report.warning(
                "NPU device",
                format!("{} is missing", device.node.display()),
                "Check the NPU kernel driver, firmware, and /dev/accel permissions.",
            );
        }
    }

    if let Some(provider) = active_provider(config, &devices) {
        let library = vendor_runtime(provider);
        if library.is_file() {
            report.pass(
                "Accelerator runtime",
                format!(
                    "{} is registered; `gaze doctor --benchmark` checks model sessions",
                    library.display()
                ),
            );
        } else {
            report.warning(
                "Accelerator runtime",
                format!("{provider} is configured but not registered"),
                register_fix(&library),
            );
        }
        return;
    }

    let mut providers: Vec<&str> = devices.iter().map(|device| device.provider).collect();
    providers.sort_unstable();
    providers.dedup();
    for provider in providers {
        let library = vendor_runtime(provider);
        if library.is_file() {
            report.off(
                "NPU acceleration",
                format!("{provider} runtime is registered; CPU is configured"),
                "Choose auto/npu in `gaze config`, then restart gazed.",
            );
        } else {
            report.off(
                "NPU acceleration",
                format!("an NPU is present but no {provider} runtime is registered"),
                format!(
                    "{} Then choose auto/npu in `gaze config`.",
                    register_fix(&library)
                ),
            );
        }
    }
}
