use gstreamer::prelude::*;
use opencv::core::Mat;
use opencv::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

use crate::config::{CameraConfig, DEFAULT_RGB_CAMERA};
use crate::ir::devices::usb_ids_of;

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

    if let Some((vid, pid)) = parse_usb_spec(source) {
        return resolve_usb_video_node(vid, pid, false);
    }

    if let Some(pos) = source.find("/dev/video") {
        let prefix_len = "/dev/video".len();
        let tail = &source[pos + prefix_len..];
        let end_digits = tail
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(tail.len());
        return Some(format!("/dev/video{}", &tail[..end_digits]));
    }

    let target = source.strip_prefix("pipewiresrc target-object=")?.trim();

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

/// A GStreamer source element, or a request to resolve a USB VID:PID to a
/// concrete V4L2 node at open time.
#[derive(Debug, PartialEq, Eq)]
enum SourceElement {
    Element(String),
    ResolveUsb {
        vid: u16,
        pid: u16,
        want_color: bool,
    },
}

/// Turn a configured `rgb`/`ir` value into a GStreamer source element.
///
/// `primary` is PipeWire (needs a session); `/dev/video<n>` and `usb:VVVV:PPPP`
/// go straight to `v4l2src`, which works in greeters that never hand out a
/// PipeWire session. `want_color` picks the color node for RGB and the mono
/// node for IR when a `usb:` spec resolves to more than one node.
fn classify_source(source: &str, want_color: bool) -> anyhow::Result<SourceElement> {
    let source = source.trim();
    if source.is_empty() {
        anyhow::bail!(
            "camera source cannot be empty; use \"primary\", \"/dev/video<n>\", \"usb:VVVV:PPPP\", or a GStreamer source"
        );
    }
    if source == DEFAULT_RGB_CAMERA {
        return Ok(SourceElement::Element("pipewiresrc".to_string()));
    }
    if let Some((vid, pid)) = parse_usb_spec(source) {
        return Ok(SourceElement::ResolveUsb {
            vid,
            pid,
            want_color,
        });
    }
    if source.starts_with("usb:") {
        anyhow::bail!("invalid USB camera spec {source:?}; expected usb:VVVV:PPPP (hex VID:PID)");
    }
    if source.starts_with("/dev/video") {
        let is_node = source
            .strip_prefix("/dev/video")
            .is_some_and(|index| !index.is_empty() && index.chars().all(|c| c.is_ascii_digit()));
        if !is_node {
            anyhow::bail!("invalid V4L2 camera node {source:?}; expected /dev/video<number>");
        }
        return Ok(SourceElement::Element(format!("v4l2src device={source}")));
    }
    Ok(SourceElement::Element(source.to_string()))
}

/// Parse a `usb:VVVV:PPPP` spec (hex VID:PID) into its numeric ids.
pub fn parse_usb_spec(source: &str) -> Option<(u16, u16)> {
    let (vid, pid) = source.trim().strip_prefix("usb:")?.split_once(':')?;
    let vid = u16::from_str_radix(vid.trim(), 16).ok()?;
    let pid = u16::from_str_radix(pid.trim(), 16).ok()?;
    Some((vid, pid))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VideoNodeInfo {
    node: String,
    vid: u16,
    pid: u16,
    is_color: bool,
}

/// Pick the `/dev/video<n>` node matching `vid:pid` whose color-ness matches the
/// caller (color for RGB, mono for IR), preferring the lowest-numbered node so
/// the choice is stable across boots.
fn select_usb_node(
    nodes: &[VideoNodeInfo],
    vid: u16,
    pid: u16,
    want_color: bool,
) -> Option<String> {
    nodes
        .iter()
        .filter(|n| n.vid == vid && n.pid == pid && n.is_color == want_color)
        .min_by_key(|n| video_node_index(&n.node).unwrap_or(u32::MAX))
        .map(|n| n.node.clone())
}

fn video_node_index(node: &str) -> Option<u32> {
    node.strip_prefix("/dev/video")?.parse().ok()
}

/// Scan V4L2 nodes for one matching `vid:pid` with the requested color-ness.
///
/// Uses the GStreamer device monitor (which enumerates through the plain V4L2
/// provider without a PipeWire session) and reads the USB ids from sysfs.
fn resolve_usb_video_node(vid: u16, pid: u16, want_color: bool) -> Option<String> {
    gstreamer::init().ok()?;
    let monitor = gstreamer::DeviceMonitor::new();
    let caps = gstreamer::Caps::builder("video/x-raw").build();
    monitor.add_filter(Some("Video/Source"), Some(&caps));
    monitor.start().ok()?;
    wait_for_device_updates(&monitor);
    let devices = monitor.devices();
    monitor.stop();

    let mut nodes = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for device in devices {
        let Some(node) = device_video_node(&device) else {
            continue;
        };
        if !seen.insert(node.clone()) {
            continue;
        }
        let Some((dev_vid, dev_pid)) = usb_ids_of(&node) else {
            continue;
        };
        let is_color = has_color_caps(&device);
        nodes.push(VideoNodeInfo {
            node,
            vid: dev_vid,
            pid: dev_pid,
            is_color,
        });
    }

    select_usb_node(&nodes, vid, pid, want_color)
}

fn device_video_node(device: &gstreamer::Device) -> Option<String> {
    let props = device.properties()?;
    if let Some(path) = string_property(&props, "api.v4l2.path")
        && path.starts_with("/dev/video")
    {
        return Some(path);
    }
    let path = string_property(&props, "device.path")?;
    path.starts_with("/dev/video").then_some(path)
}

const PRIMARY_CAMERA_DISPLAY_NAME: &str = "Primary Camera";
const DEVICE_SETTLE_TIMEOUT_MS: u64 = 100;
const INTERRUPTIBLE_POLL_TIMEOUT_MS: u64 = 100;

enum FramePoll {
    Frame(Mat),
    Timeout,
    Ended,
}

pub struct Camera {
    pipeline: gstreamer::Pipeline,
    appsink: gstreamer_app::AppSink,
}

impl Drop for Camera {
    fn drop(&mut self) {
        if let Err(err) = self.pipeline.set_state(gstreamer::State::Null) {
            warn!("Failed to stop camera pipeline: {err}");
            return;
        }

        let (result, current, pending) = self
            .pipeline
            .state(Some(gstreamer::ClockTime::from_seconds(2)));
        if let Err(err) = result {
            warn!(
                ?current,
                ?pending,
                "Camera pipeline did not stop cleanly: {err}"
            );
        } else if current != gstreamer::State::Null {
            warn!(
                ?current,
                ?pending,
                "Camera pipeline did not reach the Null state"
            );
        }
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
        Self::open_kind(camera_source, true)
    }

    pub fn open_ir(camera_source: &str) -> anyhow::Result<Self> {
        Self::open_kind(camera_source, false)
    }

    fn open_kind(camera_source: &str, want_color: bool) -> anyhow::Result<Self> {
        gstreamer::init()?;
        let src_element = match classify_source(camera_source, want_color)? {
            SourceElement::Element(element) => element,
            SourceElement::ResolveUsb {
                vid,
                pid,
                want_color,
            } => {
                let node = resolve_usb_video_node(vid, pid, want_color).ok_or_else(|| {
                    anyhow::anyhow!(
                        "no {} camera found for USB {vid:04x}:{pid:04x}",
                        if want_color { "color" } else { "IR" }
                    )
                })?;
                format!("v4l2src device={node}")
            }
        };

        let pipeline_str = format!(
            "{src_element} ! video/x-raw,pixel-aspect-ratio=1/1; image/jpeg ! decodebin ! videoconvert ! videoscale ! appsink name=gaze_sink"
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

        // Pin only height and PAR: adding width squishes 16:9 to 4:3; dropping PAR stretches it.
        let caps = gstreamer::Caps::builder("video/x-raw")
            .field("format", "BGR")
            .field("height", 480)
            .field("pixel-aspect-ratio", gstreamer::Fraction::new(1, 1))
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

    fn poll_frame(&self, timeout: gstreamer::ClockTime) -> FramePoll {
        if let Some(sample) = self.appsink.try_pull_sample(timeout) {
            return match self.sample_to_mat(&sample) {
                Ok(mat) => FramePoll::Frame(mat),
                Err(err) => {
                    warn!("Dropping camera frame: {err:#}");
                    FramePoll::Timeout
                }
            };
        }

        if self.appsink.is_eos() {
            info!("Camera stream ended (EOS)");
            return FramePoll::Ended;
        }
        if let Some(msg) = self.pipeline.bus().and_then(|bus| {
            bus.timed_pop_filtered(
                gstreamer::ClockTime::ZERO,
                &[gstreamer::MessageType::Error, gstreamer::MessageType::Eos],
            )
        }) {
            match msg.view() {
                gstreamer::MessageView::Error(err) => {
                    if let Some(src) = err.src() {
                        warn!(
                            source = %src.path_string(),
                            debug = ?err.debug(),
                            "Camera pipeline error: {}",
                            err.error()
                        );
                    } else {
                        warn!(
                            debug = ?err.debug(),
                            "Camera pipeline error: {}",
                            err.error()
                        );
                    }
                }
                _ => info!("Camera stream ended (EOS)"),
            }
            return FramePoll::Ended;
        }
        let (_, current_state, _) = self.pipeline.state(Some(gstreamer::ClockTime::ZERO));
        if current_state != gstreamer::State::Playing && current_state != gstreamer::State::Paused {
            info!("Camera pipeline stopped: {:?}", current_state);
            return FramePoll::Ended;
        }

        FramePoll::Timeout
    }

    /// Wait for the next frame while checking `stop` between short polling intervals.
    pub fn next_interruptible(&mut self, stop: &AtomicBool) -> Option<Mat> {
        while !stop.load(Ordering::Relaxed) {
            match self.poll_frame(gstreamer::ClockTime::from_mseconds(
                INTERRUPTIBLE_POLL_TIMEOUT_MS,
            )) {
                FramePoll::Frame(frame) => return Some(frame),
                FramePoll::Timeout => {}
                FramePoll::Ended => return None,
            }
        }

        None
    }
}

impl Iterator for Camera {
    type Item = Mat;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.poll_frame(gstreamer::ClockTime::from_seconds(5)) {
                FramePoll::Frame(frame) => return Some(frame),
                FramePoll::Timeout => {}
                FramePoll::Ended => return None,
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
    fn open_scales_widescreen_to_square_pixels() {
        let mut camera = Camera::open(
            "videotestsrc num-buffers=3 ! capsfilter caps=video/x-raw,width=1280,height=720",
        )
        .expect("videotestsrc pipeline");
        let frame = camera.next().expect("videotestsrc frame");
        assert_eq!(frame.rows(), 480);
        // Without the PAR pin videoscale keeps the source width (1280x480 at PAR 2/3).
        let cols = frame.cols();
        assert!(
            (853..=854).contains(&cols),
            "expected aspect-preserving width, got {cols}"
        );
    }

    #[test]
    fn iterator_ends_after_eos() {
        let mut camera = Camera::open("videotestsrc num-buffers=2").expect("videotestsrc pipeline");
        assert!(camera.next().is_some());
        assert!(camera.next().is_some());
        assert!(camera.next().is_none(), "iterator must end at EOS");
    }

    #[test]
    fn iterator_ends_on_pipeline_error() {
        let mut camera =
            Camera::open("videotestsrc ! identity error-after=2").expect("identity pipeline");
        let mut frames = 0;
        while camera.next().is_some() {
            frames += 1;
            assert!(frames < 10, "iterator must end after the pipeline errors");
        }
    }

    #[test]
    fn interruptible_read_stops_when_live_pipeline_has_no_frames() {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);

        let worker = std::thread::spawn(move || {
            let mut camera = Camera::open("appsrc is-live=true format=time")
                .expect("live pipeline without frames");
            ready_tx.send(()).expect("signal camera readiness");
            let stopped = camera.next_interruptible(&worker_stop).is_none();
            done_tx.send(stopped).expect("signal camera shutdown");
        });

        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("camera must become ready");
        std::thread::sleep(std::time::Duration::from_millis(25));
        stop.store(true, Ordering::Release);
        assert!(
            done_rx
                .recv_timeout(std::time::Duration::from_secs(1))
                .expect("camera read must observe cancellation")
        );
        worker.join().expect("camera worker must exit");
    }

    #[test]
    fn classify_source_maps_every_supported_form() {
        assert_eq!(
            classify_source("primary", true).unwrap(),
            SourceElement::Element("pipewiresrc".to_string())
        );
        assert_eq!(
            classify_source("/dev/video0", true).unwrap(),
            SourceElement::Element("v4l2src device=/dev/video0".to_string())
        );
        assert_eq!(
            classify_source("/dev/video2", false).unwrap(),
            SourceElement::Element("v4l2src device=/dev/video2".to_string())
        );
        assert_eq!(
            classify_source("usb:046d:085e", true).unwrap(),
            SourceElement::ResolveUsb {
                vid: 0x046d,
                pid: 0x085e,
                want_color: true,
            }
        );
        assert_eq!(
            classify_source("usb:046d:085e", false).unwrap(),
            SourceElement::ResolveUsb {
                vid: 0x046d,
                pid: 0x085e,
                want_color: false,
            }
        );
        assert_eq!(
            classify_source("v4l2src device=/dev/video0", true).unwrap(),
            SourceElement::Element("v4l2src device=/dev/video0".to_string())
        );
    }

    #[test]
    fn classify_source_rejects_malformed_values() {
        assert!(classify_source("", true).is_err());
        assert!(classify_source("/dev/video", true).is_err());
        assert!(classify_source("/dev/video2 ! fakesink", true).is_err());
        assert!(classify_source("usb:046d", true).is_err());
        assert!(classify_source("usb:zzzz:085e", true).is_err());
    }

    #[test]
    fn open_surfaces_malformed_sources() {
        assert!(Camera::open("/dev/video").is_err());
        assert!(Camera::open_ir("/dev/video2 ! fakesink").is_err());
        assert!(Camera::open("usb:046d").is_err());
    }

    #[test]
    fn parse_usb_spec_reads_hex_vid_pid() {
        assert_eq!(parse_usb_spec("usb:046d:085e"), Some((0x046d, 0x085e)));
        assert_eq!(parse_usb_spec("usb:04F2:B67C"), Some((0x04f2, 0xb67c)));
        assert_eq!(parse_usb_spec("pci:046d:085e"), None);
        assert_eq!(parse_usb_spec("usb:046d"), None);
        assert_eq!(parse_usb_spec("usb:zzzz:085e"), None);
        assert_eq!(parse_usb_spec("primary"), None);
    }

    #[test]
    fn select_usb_node_disambiguates_color_and_mono() {
        // Brio-style single-function UVC: one VID:PID exposes a color node and a
        // mono IR node. Also a second, color-only device.
        let nodes = vec![
            VideoNodeInfo {
                node: "/dev/video0".to_string(),
                vid: 0x046d,
                pid: 0x085e,
                is_color: true,
            },
            VideoNodeInfo {
                node: "/dev/video2".to_string(),
                vid: 0x046d,
                pid: 0x085e,
                is_color: false,
            },
            VideoNodeInfo {
                node: "/dev/video4".to_string(),
                vid: 0x1234,
                pid: 0x5678,
                is_color: true,
            },
        ];
        assert_eq!(
            select_usb_node(&nodes, 0x046d, 0x085e, true),
            Some("/dev/video0".to_string())
        );
        assert_eq!(
            select_usb_node(&nodes, 0x046d, 0x085e, false),
            Some("/dev/video2".to_string())
        );
        // No mono node on a color-only device, and unknown ids resolve to nothing.
        assert_eq!(select_usb_node(&nodes, 0x1234, 0x5678, false), None);
        assert_eq!(select_usb_node(&nodes, 0xdead, 0xbeef, true), None);
    }

    #[test]
    fn select_usb_node_prefers_lowest_numbered_node() {
        let nodes = vec![
            VideoNodeInfo {
                node: "/dev/video10".to_string(),
                vid: 0x046d,
                pid: 0x085e,
                is_color: true,
            },
            VideoNodeInfo {
                node: "/dev/video2".to_string(),
                vid: 0x046d,
                pid: 0x085e,
                is_color: true,
            },
        ];
        assert_eq!(
            select_usb_node(&nodes, 0x046d, 0x085e, true),
            Some("/dev/video2".to_string())
        );
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
