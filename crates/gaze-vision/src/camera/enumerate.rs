// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gaze_core::config::DEFAULT_RGB_CAMERA;

use super::*;

pub fn enumerate_cameras() -> anyhow::Result<Vec<(String, String)>> {
    let mut cameras = vec![CameraEntry {
        display_name: PRIMARY_CAMERA_DISPLAY_NAME.to_string(),
        target: DEFAULT_RGB_CAMERA.to_string(),
        node: None,
    }];
    cameras.extend(collect_camera_entries(Some(true))?);
    Ok(label_camera_entries(cameras))
}

pub fn enumerate_ir_cameras() -> anyhow::Result<Vec<(String, String)>> {
    let mono = collect_camera_entries(Some(false))?;
    if !mono.is_empty() {
        return Ok(label_camera_entries(mono));
    }

    let all = collect_camera_entries(None)?;
    if all.len() < 2 {
        return Ok(Vec::new());
    }
    Ok(label_camera_entries(all))
}

/// Lists the RGB cameras with a trailing "None (IR only)" entry, kept last so an unlisted
/// configured source still falls back to the primary camera at index 0.
pub fn rgb_choices() -> anyhow::Result<Vec<(String, String)>> {
    let mut options = enumerate_cameras()?;
    options.push((RGB_NONE_DISPLAY_NAME.to_string(), String::new()));
    Ok(options)
}

/// Builds the IR picker with an explicit "None" entry, keeping list indexes consistent
/// across all front ends.
pub fn ir_choices() -> Vec<(String, String)> {
    let mut options = vec![(IR_NONE_DISPLAY_NAME.to_string(), String::new())];
    options.extend(enumerate_ir_cameras().unwrap_or_default());
    options
}

pub fn is_listed_source(options: &[(String, String)], configured: &str) -> bool {
    options.iter().any(|(_, target)| target == configured)
}

pub fn source_index(options: &[(String, String)], configured: &str) -> usize {
    options
        .iter()
        .position(|(_, target)| target == configured)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CameraEntry {
    pub(super) display_name: String,
    pub(super) target: String,
    pub(super) node: Option<String>,
}

pub(super) fn label_camera_entries(entries: Vec<CameraEntry>) -> Vec<(String, String)> {
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for entry in &entries {
        *counts.entry(entry.display_name.clone()).or_default() += 1;
    }

    entries
        .into_iter()
        .map(|entry| {
            let shared = counts
                .get(&entry.display_name)
                .is_some_and(|count| *count > 1);
            match entry.node {
                Some(node) if shared => (format!("{} ({node})", entry.display_name), entry.target),
                _ => (entry.display_name, entry.target),
            }
        })
        .collect()
}

pub(super) fn collect_camera_entries(want_color: Option<bool>) -> anyhow::Result<Vec<CameraEntry>> {
    gstreamer::init()?;
    let monitor = gstreamer::DeviceMonitor::new();
    let caps = gstreamer::Caps::builder("video/x-raw").build();
    monitor.add_filter(Some("Video/Source"), Some(&caps));
    monitor.start()?;
    wait_for_device_updates(&monitor);
    let devices = monitor.devices();
    monitor.stop();

    let mut cameras: Vec<CameraEntry> = Vec::new();

    for device in devices {
        let display_name = device.display_name().to_string();
        if let Some(props) = device.properties() {
            if !props.has_name("pipewire-proplist") {
                continue;
            }
            if want_color.is_some_and(|want_color| want_color != has_color_caps(&device)) {
                continue;
            }
            let Some(target) = pipewire_target(&props) else {
                continue;
            };
            let target = format!("pipewiresrc target-object={target}");
            if !cameras.iter().any(|entry| entry.target == target) {
                cameras.push(CameraEntry {
                    display_name,
                    target,
                    node: v4l2_node_of(&props),
                });
            }
        }
    }

    Ok(cameras)
}

pub(super) fn v4l2_node_of(props: &gstreamer::StructureRef) -> Option<String> {
    string_property(props, "api.v4l2.path").or_else(|| {
        string_property(props, "device.path").filter(|path| path.starts_with("/dev/video"))
    })
}

pub(super) fn wait_for_device_updates(monitor: &gstreamer::DeviceMonitor) {
    let bus = monitor.bus();
    while bus
        .timed_pop_filtered(
            gstreamer::ClockTime::from_mseconds(DEVICE_SETTLE_TIMEOUT_MS),
            &[
                gstreamer::MessageType::DeviceAdded,
                gstreamer::MessageType::DeviceRemoved,
            ],
        )
        .is_some()
    {}
}

pub(super) fn pipewire_target(props: &gstreamer::StructureRef) -> Option<String> {
    string_property(props, "node.name")
        .or_else(|| string_property(props, "object.serial"))
        .or_else(|| string_property(props, "object.id"))
        .or_else(|| string_property(props, "object.path"))
}

pub(super) fn string_property(props: &gstreamer::StructureRef, name: &str) -> Option<String> {
    if let Ok(value) = props.get::<String>(name) {
        Some(value)
    } else if let Ok(value) = props.get::<u64>(name) {
        Some(value.to_string())
    } else if let Ok(value) = props.get::<u32>(name) {
        Some(value.to_string())
    } else {
        None
    }
}

pub(super) fn has_color_caps(device: &gstreamer::Device) -> bool {
    let Some(caps) = device.caps() else {
        return true;
    };

    let mut saw_raw_video = false;
    for structure in caps.iter() {
        if structure.name() == "image/jpeg" {
            return true;
        }
        if structure.name() != "video/x-raw" {
            continue;
        }

        saw_raw_video = true;
        let Ok(format) = structure.get::<String>("format") else {
            return true;
        };
        let format = if format == "DMA_DRM" {
            structure.get::<String>("drm-format").unwrap_or(format)
        } else {
            format
        };

        if !is_mono_format(&format) {
            return true;
        }
    }

    !saw_raw_video
}

pub(super) fn is_mono_format(format: &str) -> bool {
    let format = format.trim().to_ascii_uppercase();
    format.starts_with("GRAY")
        || format.starts_with("GREY")
        || matches!(format.as_str(), "R8" | "R16" | "Y8" | "Y16")
}
