use gstreamer::prelude::*;
use opencv::core::Mat;
use opencv::prelude::*;
use opencv::videoio::{CAP_GSTREAMER, VideoCapture};
use tracing::info;

use crate::config::{CameraConfig, DEFAULT_RGB_CAMERA};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraKind {
    Rgb,
    Ir { node: String },
}

/// Resolve the capture source and kind from config. A set `cameras.ir` wins,
/// capturing through a GStreamer `v4l2src` on that node instead of RGB.
pub fn resolve_source(cameras: &CameraConfig) -> (String, CameraKind) {
    let ir = cameras.ir.trim();
    if ir.is_empty() {
        (cameras.rgb.clone(), CameraKind::Rgb)
    } else {
        (
            format!("v4l2src device={ir}"),
            CameraKind::Ir {
                node: ir.to_string(),
            },
        )
    }
}

const PRIMARY_CAMERA_DISPLAY_NAME: &str = "Primary Camera";
const DEVICE_SETTLE_TIMEOUT_MS: u64 = 100;

pub struct Camera {
    cap: VideoCapture,
}

impl Camera {
    pub fn open(camera_source: &str) -> anyhow::Result<Self> {
        let source = camera_source.trim();
        let p = if source.is_empty() {
            anyhow::bail!("camera source cannot be empty; use \"primary\" or a GStreamer source");
        } else if source == DEFAULT_RGB_CAMERA {
            "pipewiresrc ! videoconvert ! appsink".to_string()
        } else if source.starts_with("/dev/video") {
            anyhow::bail!(
                "direct /dev/video* camera paths are not supported; use \"primary\" or a GStreamer source"
            );
        } else {
            format!("{} ! videoconvert ! appsink", source)
        };
        info!("Attempting to open GStreamer camera: {}", p);

        let cap = VideoCapture::from_file(&p, CAP_GSTREAMER)?;

        if !cap.is_opened()? {
            anyhow::bail!("Failed to open camera source {}", camera_source);
        }
        Ok(Self { cap })
    }

    pub fn capture_frame(&mut self) -> anyhow::Result<Mat> {
        let mut frame = Mat::default();
        self.cap.read(&mut frame)?;
        if frame.empty() {
            anyhow::bail!("Captured an empty frame from camera");
        }
        let mut mirrored = Mat::default();
        opencv::core::flip(&frame, &mut mirrored, 1)?;
        Ok(mirrored)
    }
}

pub fn enumerate_cameras() -> anyhow::Result<Vec<(String, String)>> {
    gstreamer::init()?;
    let monitor = gstreamer::DeviceMonitor::new();
    let caps = gstreamer::Caps::builder("video/x-raw").build();
    monitor.add_filter(Some("Video/Source"), Some(&caps));
    monitor.start()?;
    wait_for_device_updates(&monitor);
    let devices = monitor.devices();
    monitor.stop();

    let mut cameras = vec![(
        PRIMARY_CAMERA_DISPLAY_NAME.to_string(),
        DEFAULT_RGB_CAMERA.to_string(),
    )];
    for device in devices {
        let display_name = device.display_name().to_string();
        if let Some(props) = device.properties() {
            if !props.has_name("pipewire-proplist") || !has_color_caps(&device) {
                continue;
            }
            let Some(target) = pipewire_target(&props) else {
                continue;
            };
            let target = format!("pipewiresrc target-object={}", target);
            if !cameras.iter().any(|(_, t)| t == &target) {
                cameras.push((display_name, target));
            }
        }
    }

    Ok(cameras)
}

fn wait_for_device_updates(monitor: &gstreamer::DeviceMonitor) {
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

fn pipewire_target(props: &gstreamer::StructureRef) -> Option<String> {
    string_property(props, "node.name")
        .or_else(|| string_property(props, "object.serial"))
        .or_else(|| string_property(props, "object.id"))
        .or_else(|| string_property(props, "object.path"))
}

fn string_property(props: &gstreamer::StructureRef, name: &str) -> Option<String> {
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

fn has_color_caps(device: &gstreamer::Device) -> bool {
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

fn is_mono_format(format: &str) -> bool {
    let format = format.trim().to_ascii_uppercase();
    format.starts_with("GRAY")
        || format.starts_with("GREY")
        || matches!(format.as_str(), "R8" | "R16" | "Y8" | "Y16")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_source_uses_rgb_when_no_ir_configured() {
        let cameras = CameraConfig {
            rgb: "primary".to_string(),
            ir: String::new(),
            emitter_enabled: false,
            dark_luma_threshold: 70,
        };
        let (source, kind) = resolve_source(&cameras);
        assert_eq!(source, "primary");
        assert_eq!(kind, CameraKind::Rgb);
    }

    #[test]
    fn resolve_source_builds_v4l2src_pipeline_for_ir_node() {
        let cameras = CameraConfig {
            rgb: "primary".to_string(),
            ir: "/dev/video2".to_string(),
            emitter_enabled: true,
            dark_luma_threshold: 70,
        };
        let (source, kind) = resolve_source(&cameras);
        assert_eq!(source, "v4l2src device=/dev/video2");
        assert_eq!(
            kind,
            CameraKind::Ir {
                node: "/dev/video2".to_string()
            }
        );
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
}
