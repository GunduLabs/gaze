use gstreamer::prelude::*;
use opencv::core::Mat;
use opencv::prelude::*;
use tracing::info;

use crate::config::{CameraConfig, DEFAULT_RGB_CAMERA};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraKind {
    Rgb { source: String },
    Ir { source: String, node: String },
}

#[derive(Debug, Clone)]
pub struct ConfiguredCameraSources {
    pub rgb: String,
    pub ir: String,
    pub ir_node: String,
}

pub fn resolve_ir_source(cameras: &CameraConfig) -> Option<(String, String)> {
    let ir = cameras.ir.trim();
    if ir.is_empty() {
        None
    } else {
        let node = resolve_node(ir).unwrap_or_default();
        Some((ir.to_string(), node))
    }
}

pub fn resolve_rgb_source(cameras: &CameraConfig) -> Option<String> {
    let rgb = cameras.rgb.trim();
    if rgb.is_empty() {
        None
    } else {
        Some(rgb.to_string())
    }
}

pub fn resolve_configured_sources(cameras: &CameraConfig) -> ConfiguredCameraSources {
    let rgb = resolve_rgb_source(cameras).unwrap_or_default();
    let (ir, ir_node) = resolve_ir_source(cameras).unwrap_or_default();
    ConfiguredCameraSources { rgb, ir, ir_node }
}

pub fn preferred_capture_source(cameras: &CameraConfig) -> (String, bool) {
    if let Some(rgb_source) = resolve_rgb_source(cameras) {
        (rgb_source, false)
    } else if let Some((ir_source, _)) = resolve_ir_source(cameras) {
        (ir_source, true)
    } else {
        (DEFAULT_RGB_CAMERA.to_string(), false)
    }
}

pub fn resolve_source(cameras: &CameraConfig) -> (String, CameraKind) {
    if let Some((ir_source, ir_node)) = resolve_ir_source(cameras) {
        (
            ir_source.clone(),
            CameraKind::Ir {
                source: ir_source,
                node: ir_node,
            },
        )
    } else {
        let rgb_source =
            resolve_rgb_source(cameras).unwrap_or_else(|| DEFAULT_RGB_CAMERA.to_string());
        (rgb_source.clone(), CameraKind::Rgb { source: rgb_source })
    }
}

pub fn resolve_node(source: &str) -> Option<String> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }

    if let Some(pos) = source.find("/dev/video") {
        let prefix_len = "/dev/video".len();
        let tail = &source[pos + prefix_len..];
        let end_digits = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        return Some(format!("/dev/video{}", &tail[..end_digits]));
    }

    let target = if let Some(stripped) = source.strip_prefix("pipewiresrc target-object=") {
        stripped.trim()
    } else {
        return None;
    };

    let target = target.trim_matches(|c| c == '"' || c == '\'');

    gstreamer::init().ok()?;
    let monitor = gstreamer::DeviceMonitor::new();
    let caps = gstreamer::Caps::builder("video/x-raw").build();
    monitor.add_filter(Some("Video/Source"), Some(&caps));
    monitor.start().ok()?;
    wait_for_device_updates(&monitor);
    let devices = monitor.devices();
    monitor.stop();

    for device in devices {
        if let Some(props) = device.properties()
            && let Some(t) = pipewire_target(&props)
            && t == target
        {
            if let Some(path) = string_property(&props, "api.v4l2.path") {
                return Some(path);
            }
            if let Some(path) = string_property(&props, "device.path")
                && path.starts_with("/dev/video")
            {
                return Some(path);
            }
        }
    }

    None
}

const PRIMARY_CAMERA_DISPLAY_NAME: &str = "Primary Camera";
const DEVICE_SETTLE_TIMEOUT_MS: u64 = 100;

pub struct Camera {
    pipeline: gstreamer::Pipeline,
    appsink: gstreamer_app::AppSink,
}

impl Drop for Camera {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gstreamer::State::Null);
        let _ = self
            .pipeline
            .state(Some(gstreamer::ClockTime::from_seconds(2)));
    }
}

pub fn frame_to_bytes(frame: &Mat) -> anyhow::Result<Vec<u8>> {
    let sz = frame.size()?;
    let total = (sz.width * sz.height * 3) as usize;
    let mut bytes = vec![0u8; total];
    unsafe {
        std::ptr::copy_nonoverlapping(frame.data(), bytes.as_mut_ptr(), total);
    }
    Ok(bytes)
}

impl Camera {
    pub fn open(camera_source: &str) -> anyhow::Result<Self> {
        Self::open_with_direct_v4l2(camera_source, false)
    }

    pub fn open_ir(camera_source: &str) -> anyhow::Result<Self> {
        Self::open_with_direct_v4l2(camera_source, true)
    }

    fn open_with_direct_v4l2(camera_source: &str, allow_direct_v4l2: bool) -> anyhow::Result<Self> {
        gstreamer::init()?;
        let source = camera_source.trim();
        let src_element = if source.is_empty() {
            anyhow::bail!("camera source cannot be empty; use \"primary\" or a GStreamer source");
        } else if source == DEFAULT_RGB_CAMERA {
            "pipewiresrc".to_string()
        } else if source.starts_with("/dev/video") {
            let index = source
                .strip_prefix("/dev/video")
                .filter(|index| !index.is_empty() && index.chars().all(|c| c.is_ascii_digit()));
            if !allow_direct_v4l2 {
                anyhow::bail!(
                    "direct /dev/video* RGB camera paths are not supported; use \"primary\" or a GStreamer source"
                );
            } else if index.is_none() {
                anyhow::bail!("invalid V4L2 camera node {source:?}; expected /dev/video<number>");
            }
            format!("v4l2src device={source}")
        } else {
            source.to_string()
        };

        let pipeline_str = format!(
            "{src_element} ! video/x-raw; image/jpeg ! decodebin ! videoconvert ! videoscale ! appsink name=gaze_sink"
        );
        info!("Attempting to open GStreamer camera: {}", pipeline_str);

        let pipeline = gstreamer::parse::launch(&pipeline_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse pipeline for {camera_source}: {e}"))?
            .downcast::<gstreamer::Pipeline>()
            .map_err(|_| anyhow::anyhow!("Pipeline is not a gst::Pipeline"))?;

        let appsink = pipeline
            .by_name("gaze_sink")
            .ok_or_else(|| anyhow::anyhow!("appsink element not found in pipeline"))?
            .downcast::<gstreamer_app::AppSink>()
            .map_err(|_| anyhow::anyhow!("gaze_sink is not an AppSink"))?;

        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "BGR")
            .field("width", 640)
            .field("height", 480)
            .build();
        appsink.set_caps(Some(&caps));

        appsink.set_drop(true);
        appsink.set_max_buffers(1);

        pipeline
            .set_state(gstreamer::State::Playing)
            .map_err(|e| anyhow::anyhow!("Failed to start pipeline for {camera_source}: {e}"))?;

        Ok(Self { pipeline, appsink })
    }

    fn sample_to_mat(&self, sample: &gstreamer::Sample) -> anyhow::Result<Mat> {
        let buffer = sample
            .buffer()
            .ok_or_else(|| anyhow::anyhow!("Sample has no buffer"))?;
        let caps = sample
            .caps()
            .ok_or_else(|| anyhow::anyhow!("Sample has no caps"))?;

        let video_info = gstreamer_video::VideoInfo::from_caps(caps)
            .map_err(|e| anyhow::anyhow!("Failed to parse video info: {e}"))?;

        anyhow::ensure!(
            video_info.format() == gstreamer_video::VideoFormat::Bgr,
            "Expected BGR format, got {:?}",
            video_info.format()
        );

        let width = video_info.width() as usize;
        let height = video_info.height() as usize;
        let stride = video_info.stride()[0] as usize;

        let map = buffer
            .map_readable()
            .map_err(|_| anyhow::anyhow!("Buffer is not readable"))?;

        let frame = unsafe {
            opencv::core::Mat::new_rows_cols_with_data_unsafe(
                height as i32,
                width as i32,
                opencv::core::CV_8UC3,
                map.as_ptr() as *mut std::ffi::c_void,
                stride,
            )?
        };

        let mut mirrored = Mat::default();
        opencv::core::flip(&frame, &mut mirrored, 1)?;
        Ok(mirrored)
    }
}

impl Iterator for Camera {
    type Item = Mat;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(sample) = self
                .appsink
                .try_pull_sample(gstreamer::ClockTime::from_seconds(5))
            {
                if let Ok(mat) = self.sample_to_mat(&sample) {
                    return Some(mat);
                }
            } else {
                let (_, current_state, _) = self.pipeline.state(Some(gstreamer::ClockTime::ZERO));
                if current_state != gstreamer::State::Playing
                    && current_state != gstreamer::State::Paused
                {
                    info!("Camera pipeline stopped: {:?}", current_state);
                    return None;
                }
            }
        }
    }
}

pub fn enumerate_cameras() -> anyhow::Result<Vec<(String, String)>> {
    enumerate_cameras_filtered(false)
}

pub fn enumerate_ir_cameras() -> anyhow::Result<Vec<(String, String)>> {
    enumerate_cameras_filtered(true)
}

fn enumerate_cameras_filtered(mono_only: bool) -> anyhow::Result<Vec<(String, String)>> {
    gstreamer::init()?;
    let monitor = gstreamer::DeviceMonitor::new();
    let caps = gstreamer::Caps::builder("video/x-raw").build();
    monitor.add_filter(Some("Video/Source"), Some(&caps));
    monitor.start()?;
    wait_for_device_updates(&monitor);
    let devices = monitor.devices();
    monitor.stop();

    let mut cameras = if !mono_only {
        vec![(
            PRIMARY_CAMERA_DISPLAY_NAME.to_string(),
            DEFAULT_RGB_CAMERA.to_string(),
        )]
    } else {
        Vec::new()
    };

    for device in devices {
        let display_name = device.display_name().to_string();
        if let Some(props) = device.properties() {
            if !props.has_name("pipewire-proplist") {
                continue;
            }
            let is_color = has_color_caps(&device);
            if !mono_only && !is_color {
                continue;
            }
            if mono_only && is_color {
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
            dark_luma_threshold: 30,
        };
        let (source, kind) = resolve_source(&cameras);
        assert_eq!(source, "primary");
        assert_eq!(
            kind,
            CameraKind::Rgb {
                source: "primary".to_string()
            }
        );
    }

    #[test]
    fn resolve_source_builds_v4l2src_pipeline_for_ir_node() {
        let cameras = CameraConfig {
            rgb: "primary".to_string(),
            ir: "/dev/video2".to_string(),
            emitter_enabled: true,
            dark_luma_threshold: 30,
        };
        let (source, kind) = resolve_source(&cameras);
        assert_eq!(source, "/dev/video2");
        assert_eq!(
            kind,
            CameraKind::Ir {
                source: "/dev/video2".to_string(),
                node: "/dev/video2".to_string()
            }
        );
    }

    #[test]
    fn direct_v4l2_open_is_restricted_to_valid_ir_nodes() {
        assert!(Camera::open("/dev/video2").is_err());
        assert!(Camera::open_ir("/dev/video").is_err());
        assert!(Camera::open_ir("/dev/video2 ! fakesink").is_err());
    }

    #[test]
    fn resolve_source_builds_pipewiresrc_pipeline_for_ir() {
        let cameras = CameraConfig {
            rgb: "primary".to_string(),
            ir: "pipewiresrc target-object=device-name".to_string(),
            emitter_enabled: true,
            dark_luma_threshold: 30,
        };
        let (source, kind) = resolve_source(&cameras);
        assert_eq!(source, "pipewiresrc target-object=device-name");
        assert_eq!(
            kind,
            CameraKind::Ir {
                source: "pipewiresrc target-object=device-name".to_string(),
                node: String::new()
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
