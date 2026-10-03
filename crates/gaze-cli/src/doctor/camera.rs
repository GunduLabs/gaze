// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) fn detected_source_remedy(
    cameras: &[(String, String)],
    key: &str,
    automatic: Option<&str>,
) -> String {
    let detected: Vec<&str> = cameras
        .iter()
        .filter(|(_, target)| target != gaze_core::config::DEFAULT_RGB_CAMERA)
        .map(|(_, target)| target.as_str())
        .collect();

    if detected.is_empty() {
        return match automatic {
            Some(automatic) => format!(
                "No PipeWire source is currently advertised. Reconnect the camera, then set \
                 {key} to a detected source, or to \"{automatic}\" to resolve it at runtime."
            ),
            None => format!(
                "No PipeWire source is currently advertised. Reconnect the camera, then set \
                 {key} to a detected source."
            ),
        };
    }

    format!(
        "Run `gaze config` to pick one interactively, or set {key} to one of the detected \
         sources: {}",
        detected
            .iter()
            .map(|target| format!("\"{target}\""))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub(super) fn gstreamer_package_hint_for(os_release: &str) -> &'static str {
    let os_release = os_release.to_ascii_lowercase();
    if ["arch", "manjaro", "omarchy"]
        .iter()
        .any(|family| os_release.contains(family))
    {
        "Install them with `sudo pacman -S gst-plugins-base gst-plugins-good gst-plugin-pipewire`."
    } else if ["debian", "ubuntu"]
        .iter()
        .any(|family| os_release.contains(family))
    {
        "Install them with `sudo apt install gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-pipewire`."
    } else if ["fedora", "rhel", "centos"]
        .iter()
        .any(|family| os_release.contains(family))
    {
        "Install them with `sudo dnf install gstreamer1-plugins-base gstreamer1-plugins-good pipewire-gstreamer`."
    } else if os_release.contains("suse") {
        "Install them with `sudo zypper install gstreamer-plugins-base gstreamer-plugins-good gstreamer-plugin-pipewire`."
    } else {
        "Install the GStreamer base, good, and PipeWire plugin packages for this distribution."
    }
}

pub(super) fn gstreamer_package_hint() -> &'static str {
    let os_release = fs::read_to_string("/etc/os-release").unwrap_or_default();
    gstreamer_package_hint_for(&os_release)
}

pub(super) fn check_gstreamer_plugins(report: &mut Report) -> bool {
    match gaze_vision::camera::missing_camera_elements() {
        Ok(missing) if missing.is_empty() => {
            report.pass(
                "GStreamer plugins",
                "base, JPEG/V4L2, and PipeWire camera elements are available",
            );
            true
        }
        Ok(missing) => {
            report.error(
                "GStreamer plugins",
                format!(
                    "required camera elements are missing: {}",
                    missing.join(", ")
                ),
                gstreamer_package_hint(),
            );
            false
        }
        Err(err) => {
            report.error(
                "GStreamer plugins",
                format!("the plugin registry could not be initialized: {err}"),
                gstreamer_package_hint(),
            );
            false
        }
    }
}

pub(super) fn check_cameras(report: &mut Report, config: Option<&Config>) {
    if !check_gstreamer_plugins(report) {
        return;
    }

    let Some(config) = config else {
        return;
    };

    let rgb = config.cameras.rgb.trim();
    if !rgb.is_empty() {
        match gaze_vision::camera::enumerate_cameras() {
            Ok(cameras) => {
                let detected = cameras
                    .iter()
                    .filter(|(_, target)| target != gaze_core::config::DEFAULT_RGB_CAMERA)
                    .count();
                if rgb == gaze_core::config::DEFAULT_RGB_CAMERA {
                    if detected > 0 {
                        report.pass(
                            "RGB camera",
                            format!("{detected} color camera(s) visible through PipeWire"),
                        );
                    } else {
                        report.warning(
                            "RGB camera",
                            "no physical color camera was advertised by PipeWire",
                            "Check camera privacy controls and run `gaze config` from the local desktop session.",
                        );
                    }
                } else if rgb.starts_with("pipewiresrc target-object=") {
                    if cameras.iter().any(|(_, target)| target == rgb) {
                        report.pass("RGB camera", "the configured PipeWire source is visible");
                    } else {
                        report.error(
                            "RGB camera",
                            format!("configured source is not visible: {rgb}"),
                            detected_source_remedy(
                                &cameras,
                                "cameras.rgb",
                                Some(gaze_core::config::DEFAULT_RGB_CAMERA),
                            ),
                        );
                    }
                } else if let Some((vid, pid)) = gaze_vision::camera::parse_usb_spec(rgb) {
                    report.pass(
                        "RGB camera",
                        format!("resolves the color node for USB {vid:04x}:{pid:04x} at runtime"),
                    );
                } else if rgb.starts_with("/dev/video") {
                    match fs::metadata(rgb) {
                        Ok(metadata) if metadata.file_type().is_char_device() => {
                            report.pass("RGB camera", format!("{rgb} is a character device"));
                        }
                        Ok(_) => report.error(
                            "RGB camera",
                            format!("{rgb} is not a character device"),
                            "Point cameras.rgb at a /dev/video* node.",
                        ),
                        Err(err) => report.error(
                            "RGB camera",
                            format!("{rgb} is not accessible: {err}"),
                            "Check the device path and permissions.",
                        ),
                    }
                } else {
                    report.warning(
                        "RGB camera",
                        "a custom GStreamer source is configured and was not opened by this read-only check",
                        "Run `gaze auth` to verify that the custom source produces frames.",
                    );
                }
            }
            Err(err) => report.error(
                "RGB camera",
                format!("GStreamer camera enumeration failed: {err}"),
                "Verify the GStreamer PipeWire plugin is installed and PipeWire is running.",
            ),
        }
    }

    let ir = config.cameras.ir.trim();
    if ir.is_empty() {
        return;
    }
    if ir.starts_with("/dev/video") {
        match fs::metadata(ir) {
            Ok(metadata) if metadata.file_type().is_char_device() => {
                report.pass("IR camera", format!("{ir} is a character device"));
            }
            Ok(_) => report.error(
                "IR camera",
                format!("{ir} is not a device node"),
                "Choose the IR camera's /dev/video* node.",
            ),
            Err(err) => report.error(
                "IR camera",
                format!("cannot access {ir}: {err}"),
                "Correct cameras.ir or reconnect the IR camera.",
            ),
        }
    } else if ir.starts_with("pipewiresrc target-object=") {
        match gaze_vision::camera::enumerate_ir_cameras() {
            Ok(cameras) if cameras.iter().any(|(_, target)| target == ir) => {
                report.pass("IR camera", "the configured PipeWire source is visible");
            }
            Ok(cameras) => report.error(
                "IR camera",
                format!("configured source is not visible: {ir}"),
                detected_source_remedy(&cameras, "cameras.ir", None),
            ),
            Err(err) => report.error(
                "IR camera",
                format!("GStreamer IR camera enumeration failed: {err}"),
                "Verify PipeWire is running and the IR device is connected.",
            ),
        }
    } else if let Some((vid, pid)) = gaze_vision::camera::parse_usb_spec(ir) {
        report.pass(
            "IR camera",
            format!("resolves the IR node for USB {vid:04x}:{pid:04x} at runtime"),
        );
    } else {
        report.warning(
            "IR camera",
            "a custom GStreamer source is configured and was not opened by this read-only check",
            "Run `gaze auth` to verify that the IR source produces frames.",
        );
    }

    if config.cameras.emitter_enabled {
        check_i2c_emitter(report, ir);
    }
}

pub(super) fn check_i2c_emitter(report: &mut Report, ir: &str) {
    let Some(node) = gaze_vision::camera::resolve_node(ir) else {
        return;
    };
    match gaze_core::ir::i2c::I2cEmitter::diagnose(&node) {
        None => {}
        Some(Ok(emitter)) => report.pass(
            "IR emitter",
            format!("{} matches {node} on {}", emitter.name(), emitter.bus()),
        ),
        Some(Err(reason)) => report.warning(
            "IR emitter",
            format!("the I2C emitter profile for {node} does not apply: {reason}"),
            "Load the i2c-dev module, make sure the IR bridge is running, and check that the \
             sensor driver is bound. Authentication continues without illumination until then.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gstreamer_plugin_remedies_use_distro_package_names() {
        for (os_release, packages) in [
            (
                "ID=omarchy\nID_LIKE=arch\n",
                [
                    "gst-plugins-base",
                    "gst-plugins-good",
                    "gst-plugin-pipewire",
                ],
            ),
            (
                "ID=ubuntu\nID_LIKE=debian\n",
                [
                    "gstreamer1.0-plugins-base",
                    "gstreamer1.0-plugins-good",
                    "gstreamer1.0-pipewire",
                ],
            ),
            (
                "ID=fedora\n",
                [
                    "gstreamer1-plugins-base",
                    "gstreamer1-plugins-good",
                    "pipewire-gstreamer",
                ],
            ),
            (
                "ID=opensuse-tumbleweed\nID_LIKE=suse\n",
                [
                    "gstreamer-plugins-base",
                    "gstreamer-plugins-good",
                    "gstreamer-plugin-pipewire",
                ],
            ),
        ] {
            let hint = gstreamer_package_hint_for(os_release);
            for package in packages {
                assert!(hint.contains(package), "{hint:?} does not name {package}");
            }
        }
    }

    #[test]
    fn camera_remedy_lists_detected_sources() {
        let cameras = vec![
            (
                "Primary camera".to_string(),
                gaze_core::config::DEFAULT_RGB_CAMERA.to_string(),
            ),
            (
                "Integrated Camera".to_string(),
                "pipewiresrc target-object=v4l2_input.pci-0000_00_14_0".to_string(),
            ),
        ];

        let remedy = detected_source_remedy(
            &cameras,
            "cameras.rgb",
            Some(gaze_core::config::DEFAULT_RGB_CAMERA),
        );
        assert!(remedy.contains("cameras.rgb"), "{remedy}");
        assert!(
            remedy.contains("\"pipewiresrc target-object=v4l2_input.pci-0000_00_14_0\""),
            "the detected source must be quoted verbatim for copy-paste: {remedy}"
        );
        assert!(
            !remedy.contains("\"primary\""),
            "the primary pseudo-source is not a selectable node: {remedy}"
        );
    }

    #[test]
    fn camera_remedy_only_offers_primary_where_it_is_valid() {
        let none_detected = vec![(
            "Primary camera".to_string(),
            gaze_core::config::DEFAULT_RGB_CAMERA.to_string(),
        )];

        let rgb = detected_source_remedy(
            &none_detected,
            "cameras.rgb",
            Some(gaze_core::config::DEFAULT_RGB_CAMERA),
        );
        assert!(rgb.contains("cameras.rgb"), "{rgb}");
        assert!(rgb.contains("\"primary\""), "{rgb}");

        let ir = detected_source_remedy(&[], "cameras.ir", None);
        assert!(ir.contains("cameras.ir"), "{ir}");
        assert!(
            !ir.contains("primary"),
            "cameras.ir has no primary fallback: {ir}"
        );
    }
}
