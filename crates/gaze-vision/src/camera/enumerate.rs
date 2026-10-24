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

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(display_name: &str, target: &str, node: Option<&str>) -> CameraEntry {
        CameraEntry {
            display_name: display_name.to_string(),
            target: format!("pipewiresrc target-object={target}"),
            node: node.map(str::to_string),
        }
    }

    #[test]
    fn shared_display_names_are_labelled_with_their_v4l2_node() {
        let labelled = label_camera_entries(vec![
            entry("ASUS 5M webcam (V4L2)", "cam0", Some("/dev/video0")),
            entry("ASUS 5M webcam (V4L2)", "cam2", Some("/dev/video2")),
        ]);

        assert_eq!(
            labelled
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "ASUS 5M webcam (V4L2) (/dev/video0)",
                "ASUS 5M webcam (V4L2) (/dev/video2)"
            ]
        );
        assert_eq!(labelled[1].1, "pipewiresrc target-object=cam2");
    }

    #[test]
    fn a_unique_display_name_is_left_alone() {
        let labelled = label_camera_entries(vec![
            entry("Integrated Webcam", "cam0", Some("/dev/video0")),
            entry("IR Camera", "cam2", Some("/dev/video2")),
        ]);

        assert_eq!(labelled[0].0, "Integrated Webcam");
        assert_eq!(labelled[1].0, "IR Camera");
    }

    #[test]
    fn shared_display_names_without_a_node_stay_as_they_are() {
        let labelled = label_camera_entries(vec![
            entry("ASUS 5M webcam (V4L2)", "cam0", None),
            entry("ASUS 5M webcam (V4L2)", "cam2", Some("/dev/video2")),
        ]);

        assert_eq!(labelled[0].0, "ASUS 5M webcam (V4L2)");
        assert_eq!(labelled[1].0, "ASUS 5M webcam (V4L2) (/dev/video2)");
    }

    #[test]
    fn mono_format_detection_is_case_and_whitespace_insensitive() {
        for format in [
            "GRAY8", " gray16 ", "GREY", "grey12", "R8", "r16", "Y8", " y16 ",
        ] {
            assert!(is_mono_format(format), "{format} should be mono");
        }

        for format in ["RGB", "BGR", "RGBA", "YUY2", "NV12", "DMA_DRM", ""] {
            assert!(!is_mono_format(format), "{format} should be color/unknown");
        }
    }

    fn sample_options() -> Vec<(String, String)> {
        vec![
            (IR_NONE_DISPLAY_NAME.to_string(), String::new()),
            ("Integrated IR".to_string(), "/dev/video2".to_string()),
        ]
    }

    #[test]
    fn source_index_resolves_configured_targets() {
        let options = sample_options();
        assert_eq!(source_index(&options, ""), 0);
        assert_eq!(source_index(&options, "/dev/video2"), 1);
    }

    #[test]
    fn source_index_falls_back_for_unlisted_targets() {
        let options = sample_options();
        assert!(!is_listed_source(&options, "/dev/video99"));
        assert_eq!(source_index(&options, "/dev/video99"), 0);
    }
}
