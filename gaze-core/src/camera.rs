use gstreamer::prelude::*;
use opencv::core::Mat;
use opencv::prelude::*;
use opencv::videoio::{CAP_GSTREAMER, VideoCapture};
use tracing::info;

use crate::config::{Config, DEFAULT_IR_CAMERA, DEFAULT_RGB_CAMERA};

const PRIMARY_CAMERA_DISPLAY_NAME: &str = "Primary Camera";
const DEVICE_SETTLE_TIMEOUT_MS: u64 = 100;

pub struct Camera {
    cap: VideoCapture,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraInfo {
    pub name: String,
    pub source: String,
    pub is_ir: bool,
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
    Ok(enumerate_camera_infos()?
        .into_iter()
        .filter(|camera| !camera.is_ir)
        .map(|camera| (camera.name, camera.source))
        .collect())
}

pub fn enumerate_ir_cameras() -> anyhow::Result<Vec<(String, String)>> {
    Ok(enumerate_camera_infos()?
        .into_iter()
        .filter(|camera| camera.is_ir)
        .map(|camera| (camera.name, camera.source))
        .collect())
}

pub fn enumerate_camera_infos() -> anyhow::Result<Vec<CameraInfo>> {
    gstreamer::init()?;
    let monitor = gstreamer::DeviceMonitor::new();
    let caps = gstreamer::Caps::builder("video/x-raw").build();
    monitor.add_filter(Some("Video/Source"), Some(&caps));
    monitor.start()?;
    wait_for_device_updates(&monitor);
    let devices = monitor.devices();
    monitor.stop();

    let mut cameras = vec![CameraInfo {
        name: PRIMARY_CAMERA_DISPLAY_NAME.to_string(),
        source: DEFAULT_RGB_CAMERA.to_string(),
        is_ir: false,
    }];
    for device in devices {
        let display_name = device.display_name().to_string();
        if let Some(props) = device.properties() {
            if !props.has_name("pipewire-proplist") {
                continue;
            }
            let Some(target) = pipewire_target(&props) else {
                continue;
            };
            let is_ir = is_ir_device(&device, &props);
            if !is_ir && !has_color_caps(&device) {
                continue;
            }
            let target = format!("pipewiresrc target-object={}", target);
            if !cameras.iter().any(|camera| camera.source == target) {
                let name = if is_ir {
                    format!("{display_name} [IR]")
                } else {
                    display_name
                };
                cameras.push(CameraInfo {
                    name,
                    source: target,
                    is_ir,
                });
            }
        }
    }

    Ok(cameras)
}

pub fn auth_camera_source(config: &Config) -> anyhow::Result<String> {
    if config.security.require_ir {
        resolve_camera_source(&config.cameras.ir)
    } else {
        Ok(config.cameras.rgb.clone())
    }
}

pub fn resolve_camera_source(source: &str) -> anyhow::Result<String> {
    let source = source.trim();
    if source == DEFAULT_IR_CAMERA {
        resolve_ir_camera_source(source)
    } else {
        Ok(source.to_string())
    }
}

pub fn resolve_ir_camera_source(source: &str) -> anyhow::Result<String> {
    let source = source.trim();
    if source.is_empty() || source == DEFAULT_IR_CAMERA {
        return auto_detect_ir_camera_source();
    }

    if source == DEFAULT_RGB_CAMERA {
        anyhow::bail!("primary is not an IR camera source; set cameras.ir to an IR source");
    }

    let cameras = enumerate_camera_infos().unwrap_or_default();
    if let Some(camera) = cameras.iter().find(|camera| camera.source == source) {
        if camera.is_ir {
            return Ok(source.to_string());
        }
        anyhow::bail!("configured IR camera source is not detected as IR: {source}");
    }

    Ok(source.to_string())
}

fn auto_detect_ir_camera_source() -> anyhow::Result<String> {
    enumerate_camera_infos()?
        .into_iter()
        .find(|camera| camera.is_ir)
        .map(|camera| camera.source)
        .ok_or_else(|| anyhow::anyhow!("IR camera required but no IR camera was detected"))
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

fn is_ir_device(device: &gstreamer::Device, props: &gstreamer::StructureRef) -> bool {
    has_ir_name(device, props) || has_ir_caps(device)
}

fn has_ir_name(device: &gstreamer::Device, props: &gstreamer::StructureRef) -> bool {
    let mut names = vec![device.display_name().to_string()];
    for name in [
        "api.v4l2.path",
        "device.description",
        "device.name",
        "node.description",
        "node.name",
        "object.path",
    ] {
        if let Some(value) = string_property(props, name) {
            names.push(value);
        }
    }

    names.into_iter().any(|name| has_ir_text(&name))
}

fn has_ir_text(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("infrared")
        || name
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|token| token == "ir")
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

fn has_ir_caps(device: &gstreamer::Device) -> bool {
    let Some(caps) = device.caps() else {
        return false;
    };

    caps.iter().any(|structure| {
        if structure.name() != "video/x-raw" {
            return false;
        }

        let Ok(format) = structure.get::<String>("format") else {
            return false;
        };
        let format = if format == "DMA_DRM" {
            structure.get::<String>("drm-format").unwrap_or(format)
        } else {
            format
        };

        is_mono_format(&format)
    })
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

    #[test]
    fn ir_name_detection_requires_ir_token_or_infrared() {
        for name in [
            "IR Camera",
            "Integrated IR Camera",
            "Infrared Camera",
            "video-ir",
        ] {
            assert!(has_ir_text(name), "{name} should be detected as IR");
        }

        for name in [
            "Primary Camera",
            "Virtual Camera",
            "WireCam",
            "Front Camera",
        ] {
            assert!(!has_ir_text(name), "{name} should not be detected as IR");
        }
    }
}
