// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gstreamer::prelude::*;
use opencv::core::Mat;
use opencv::prelude::*;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{info, warn};

use crate::config::{CameraConfig, DEFAULT_RGB_CAMERA};
use crate::ir::devices::{camera_function_of, find_device, usb_ids_of};

const REALTEK_IR_YUY2_WIDTH: u32 = 640;
const REALTEK_IR_YUY2_HEIGHT: u32 = 480;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraKind {
    Rgb { source: String },
    Ir { source: String, node: String },
}

fn requires_forced_ir_yuy2(vid: u16, pid: u16, want_color: bool) -> bool {
    !want_color && find_device(vid, pid).is_some_and(|device| device.requires_ir_yuy2)
}

/// The quirk belongs to the USB device, not to how the source was spelled, so it is looked up
/// from the resolved node; `usb:VVVV:PPPP`, `/dev/videoN`, and PipeWire all reach one profile.
fn node_requires_forced_ir_yuy2(node: &str, want_color: bool) -> bool {
    !want_color
        && usb_ids_of(node).is_some_and(|(vid, pid)| requires_forced_ir_yuy2(vid, pid, want_color))
}

#[derive(Debug, Clone)]
pub struct ConfiguredCameraSources {
    pub rgb: String,
    pub ir: String,
    pub ir_node: String,
    pub serial_capture: bool,
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
    let serial_capture = match cameras.parallel_capture() {
        "always" => false,
        "auto" => {
            let rgb_node = resolve_node_for(&rgb, true);
            capture_must_serialize(rgb_node.as_deref(), non_empty(&ir_node))
        }
        _ => true,
    };
    ConfiguredCameraSources {
        rgb,
        ir,
        ir_node,
        serial_capture,
    }
}

fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

fn capture_must_serialize(rgb_node: Option<&str>, ir_node: Option<&str>) -> bool {
    let Some(ir) = ir_node else {
        return true;
    };
    match rgb_node {
        Some(rgb) if rgb == ir => true,
        Some(rgb) => functions_must_serialize(camera_function_of(rgb), camera_function_of(ir)),
        None => serialize_for_unresolved_rgb(
            &color_camera_functions(),
            camera_function_of(ir).as_deref(),
        ),
    }
}

fn functions_must_serialize(rgb_function: Option<String>, ir_function: Option<String>) -> bool {
    match (rgb_function, ir_function) {
        (Some(rgb), Some(ir)) => rgb == ir,
        _ => true,
    }
}

fn color_camera_functions() -> Vec<String> {
    v4l2_nodes_with_color(true)
        .iter()
        .filter_map(|node| camera_function_of(node))
        .collect()
}

fn serialize_for_unresolved_rgb(color_functions: &[String], ir_function: Option<&str>) -> bool {
    let Some(ir_function) = ir_function else {
        return true;
    };
    color_functions.is_empty()
        || color_functions
            .iter()
            .any(|function| function == ir_function)
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

pub fn preview_can_be_shared(cameras: &CameraConfig) -> bool {
    if !cameras.ir.trim().is_empty() {
        return false;
    }
    let (source, _) = preferred_capture_source(cameras);
    is_pipewire_source(&source)
}

fn is_pipewire_source(source: &str) -> bool {
    matches!(
        classify_source(source, true),
        Ok(SourceElement::Element(element))
            if element == "pipewiresrc" || element.starts_with("pipewiresrc ")
    )
}

pub fn resolve_node(source: &str) -> Option<String> {
    resolve_node_for(source, false)
}

pub fn resolve_node_for(source: &str, want_color: bool) -> Option<String> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }

    if let Some((vid, pid)) = parse_usb_spec(source) {
        return resolve_usb_video_node(vid, pid, want_color);
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
            && let Some(path) = v4l2_node_of(&props)
        {
            return Some(path);
        }
    }

    // Without a PipeWire session the target names no device the monitor saw, but udev still
    // links the node it was named after.
    node_from_pipewire_target(target, want_color)
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

/// Turn a configured `rgb`/`ir` value into a GStreamer source element. `primary` needs a PipeWire
/// session; node and `usb:` specs use `v4l2src`, which still works in greeters that have none.
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

/// Pick the `/dev/video<n>` node for `vid:pid` with the requested color-ness, lowest-numbered
/// first so the choice stays stable across boots.
fn select_usb_node(
    nodes: &[VideoNodeInfo],
    vid: u16,
    pid: u16,
    want_color: bool,
) -> Option<String> {
    let matching_nodes = || nodes.iter().filter(|n| n.vid == vid && n.pid == pid);

    matching_nodes()
        .filter(|n| n.is_color == want_color)
        .min_by_key(|n| video_node_index(&n.node).unwrap_or(u32::MAX))
        .map(|n| n.node.clone())
        .or_else(|| {
            // Single-node Windows Hello cameras expose RGB and IR through one UVC node, so no
            // mono node is advertised. Elsewhere a missing mono node means IR really is absent.
            if want_color || !requires_forced_ir_yuy2(vid, pid, want_color) {
                return None;
            }

            matching_nodes()
                .min_by_key(|n| video_node_index(&n.node).unwrap_or(u32::MAX))
                .map(|n| n.node.clone())
        })
}

fn video_node_index(node: &str) -> Option<u32> {
    node.strip_prefix("/dev/video")?.parse().ok()
}

/// Scan V4L2 nodes for one matching `vid:pid` with the requested color-ness. The GStreamer device
/// monitor enumerates via the plain V4L2 provider without a PipeWire session; ids come from sysfs.
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

fn first_v4l2_node(want_color: bool) -> Option<String> {
    v4l2_nodes_with_color(want_color).into_iter().next()
}

/// Every V4L2 node with the requested color-ness, lowest-numbered first. Enumeration goes through
/// the plain V4L2 provider, so it still answers in a greeter that has no PipeWire session.
fn v4l2_nodes_with_color(want_color: bool) -> Vec<String> {
    if gstreamer::init().is_err() {
        return Vec::new();
    }
    let monitor = gstreamer::DeviceMonitor::new();
    let caps = gstreamer::Caps::builder("video/x-raw").build();
    monitor.add_filter(Some("Video/Source"), Some(&caps));
    if monitor.start().is_err() {
        return Vec::new();
    }
    wait_for_device_updates(&monitor);
    let devices = monitor.devices();
    monitor.stop();

    let mut seen = std::collections::HashSet::new();
    let mut nodes: Vec<String> = devices
        .iter()
        .filter_map(|device| {
            let node = device_video_node(device)?;
            if !seen.insert(node.clone()) {
                return None;
            }
            (has_color_caps(device) == want_color).then_some(node)
        })
        .collect();
    nodes.sort_by_key(|node| video_node_index(node).unwrap_or(u32::MAX));
    nodes
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
pub const IR_NONE_DISPLAY_NAME: &str = "None";
const DEVICE_SETTLE_TIMEOUT_MS: u64 = 100;
const INTERRUPTIBLE_POLL_TIMEOUT_MS: u64 = 100;
/// Long enough for a busy device to reject the stream, short enough not to sit through a live
/// source that has simply not produced its first buffer. Later failures are the frame loop's.
const PIPELINE_START_TIMEOUT_MS: u64 = 500;

enum FramePoll {
    Frame(Mat),
    Timeout,
    Ended,
}

/// An open connection to one user's PipeWire socket, passed to `pipewiresrc` as `fd=` because
/// its `pw_core` cache is keyed on that fd: the shared default `-1` would merge two sessions.
pub struct PipeWireSession(UnixStream);

impl PipeWireSession {
    pub fn connect_for_uid(uid: u32) -> std::io::Result<Self> {
        UnixStream::connect(format!("/run/user/{uid}/pipewire-0")).map(Self)
    }

    fn raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

/// The uid whose PipeWire session capture attaches to, or `None` to resolve a socket from the
/// environment. Only the uid is shared; each pipeline opens its own socket at open time.
static PIPEWIRE_UID: Mutex<Option<u32>> = Mutex::new(None);

thread_local! {
    /// The uid a capture thread was started for, which outranks the default. A thread still on
    /// its way to opening a device must not follow a rebind by the claim that preempted it.
    static PIPEWIRE_UID_FOR_THREAD: std::cell::Cell<Option<Option<u32>>> =
        const { std::cell::Cell::new(None) };
}

pub fn set_pipewire_uid(uid: Option<u32>) {
    let mut slot = PIPEWIRE_UID.lock().unwrap_or_else(|e| e.into_inner());
    *slot = uid;
}

pub fn bind_pipewire_uid_for_thread(uid: Option<u32>) {
    PIPEWIRE_UID_FOR_THREAD.set(Some(uid));
}

fn current_pipewire_uid() -> Option<u32> {
    PIPEWIRE_UID_FOR_THREAD
        .get()
        .unwrap_or_else(|| *PIPEWIRE_UID.lock().unwrap_or_else(|e| e.into_inner()))
}

/// V4L2 lets several processes open a camera and rejects the second one that tries to stream,
/// so a device held elsewhere fails during negotiation rather than at open.
fn device_is_busy(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("busy") || detail.contains("ebusy")
}

/// The state-change error says only that an element failed; the reason is on the bus.
fn bus_error_detail(pipeline: &gstreamer::Pipeline) -> Option<String> {
    let bus = pipeline.bus()?;
    while let Some(msg) = bus.pop() {
        if let gstreamer::MessageView::Error(err) = msg.view() {
            let debug = err.debug().unwrap_or_default();
            return Some(format!("{}: {debug}", err.error()));
        }
    }
    None
}

/// What a failed PipeWire open may retry through. Only the bare element, which is what `primary`
/// resolves to, means "any camera you can reach". A named target means one camera, so it may only
/// retry through that same camera's own V4L2 node; substituting another camera is not a fallback.
#[derive(Debug, PartialEq, Eq)]
enum V4l2Fallback {
    AnyDevice,
    SameNode(String),
    None,
}

fn v4l2_fallback_for(src_element: &str) -> V4l2Fallback {
    if src_element == "pipewiresrc" {
        return V4l2Fallback::AnyDevice;
    }
    let Some(fields) = src_element.strip_prefix("pipewiresrc ") else {
        return V4l2Fallback::None;
    };
    fields
        .split_whitespace()
        .find_map(|field| field.strip_prefix("target-object="))
        .map(|target| target.trim_matches(|c| c == '"' || c == '\''))
        .filter(|target| !target.is_empty())
        .map_or(V4l2Fallback::None, |target| {
            V4l2Fallback::SameNode(target.to_string())
        })
}

const V4L2_BY_PATH_DIR: &str = "/dev/v4l/by-path";

/// PipeWire names a V4L2 camera `v4l2_input.<udev ID_PATH>` with `:` rewritten as `_`, and udev
/// links that same path under `/dev/v4l/by-path`, so a pinned target names a node the kernel can
/// still reach with no PipeWire session to ask.
fn v4l2_by_path_for_target(target: &str) -> Option<String> {
    let by_path = target.strip_prefix("v4l2_input.")?;
    (!by_path.is_empty()).then(|| by_path.replace('_', ":"))
}

/// The nodes udev links for one `by-path`, lowest `video-index` first.
fn nodes_for_by_path(by_path: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(V4L2_BY_PATH_DIR) else {
        return Vec::new();
    };

    let mut indexed: Vec<(u32, String)> = entries
        .flatten()
        .filter_map(|entry| {
            let link = entry.file_name().into_string().ok()?;
            let index = link
                .strip_prefix(by_path)?
                .strip_prefix("-video-index")?
                .parse()
                .ok()?;
            let node = std::fs::canonicalize(entry.path()).ok()?;
            Some((index, node.to_str()?.to_string()))
        })
        .collect();
    indexed.sort();

    let mut seen = std::collections::HashSet::new();
    indexed
        .into_iter()
        .filter_map(|(_, node)| seen.insert(node.clone()).then_some(node))
        .collect()
}

/// The V4L2 node behind a pinned PipeWire target. A device that links several nodes also links
/// ones that cannot be captured from, such as a metadata node, so the caps decide between them.
fn node_from_pipewire_target(target: &str, want_color: bool) -> Option<String> {
    let nodes = nodes_for_by_path(&v4l2_by_path_for_target(target)?);
    if let [only] = nodes.as_slice() {
        return Some(only.clone());
    }
    let capturable = v4l2_nodes_with_color(want_color);
    nodes
        .iter()
        .find(|node| capturable.contains(node))
        .or_else(|| nodes.first())
        .cloned()
}

/// Attach `fd=` to a `pipewiresrc` element so it connects to the caller's session instead of
/// resolving a socket from the environment. Anything else is passed through untouched.
fn bind_pipewire_fd(element: &str, fd: RawFd) -> String {
    let is_pipewire = element == "pipewiresrc" || element.starts_with("pipewiresrc ");
    // A descriptor the caller spelled out wins; appending a second `fd=` would leave the
    // element carrying two values for one property.
    let already_bound = element
        .split_whitespace()
        .any(|field| field.starts_with("fd="));
    if is_pipewire && !already_bound {
        format!("{element} fd={fd}")
    } else {
        element.to_string()
    }
}

pub struct Camera {
    pipeline: gstreamer::Pipeline,
    appsink: gstreamer_app::AppSink,
    /// Keeps the socket open while the pipeline can use it. GStreamer caches its `pw_core` by
    /// fd number, so recycling the number early could hand a later capture this one's core.
    _pipewire: Option<PipeWireSession>,
    /// Why the stream ended, when the pipeline said. A frame loop that just stops looks the same
    /// as one that never saw a face, which is the wrong thing to tell the user.
    stream_error: Option<String>,
    fps: Mutex<Option<f64>>,
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
        let (src_element, force_ir_yuy2) = match classify_source(camera_source, want_color)? {
            SourceElement::Element(element) => {
                // `/dev/videoN` and PipeWire targets never carry USB ids, so
                // resolve the node they name before checking for the quirk.
                let force = !want_color
                    && resolve_node(camera_source)
                        .is_some_and(|node| node_requires_forced_ir_yuy2(&node, want_color));
                (element, force)
            }
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
                (
                    format!("v4l2src device={node}"),
                    requires_forced_ir_yuy2(vid, pid, want_color),
                )
            }
        };

        // A claim binds capture to one user's PipeWire session; without one, `pipewiresrc`
        // resolves the socket from the environment as it does inside a user's own session.
        let is_pipewire = src_element == "pipewiresrc" || src_element.starts_with("pipewiresrc ");
        let session = match (is_pipewire, current_pipewire_uid()) {
            (true, Some(uid)) => match PipeWireSession::connect_for_uid(uid) {
                Ok(session) => Some(session),
                Err(err) => {
                    // Greeters without a user manager have no socket. Carry on unbound so the
                    // V4L2 fallback below still gets its turn.
                    warn!("No PipeWire socket for uid {uid} ({err}); capture will use V4L2");
                    None
                }
            },
            _ => None,
        };
        let bound = match &session {
            Some(session) => bind_pipewire_fd(&src_element, session.raw_fd()),
            None => src_element.clone(),
        };

        let fallback = v4l2_fallback_for(&src_element);
        match Self::open_source_element(&bound, camera_source, force_ir_yuy2, session) {
            Ok(camera) => Ok(camera),
            Err(err) if fallback != V4l2Fallback::None => {
                warn!("Opening the PipeWire camera failed ({err:#}); trying a direct V4L2 device");
                let node = match &fallback {
                    V4l2Fallback::AnyDevice => first_v4l2_node(want_color),
                    V4l2Fallback::SameNode(target) => node_from_pipewire_target(target, want_color),
                    V4l2Fallback::None => None,
                }
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "PipeWire camera failed and no V4L2 fallback device was found: {err}"
                    )
                })?;
                info!("Falling back to V4L2 camera node {node}");
                let force_ir_yuy2 = node_requires_forced_ir_yuy2(&node, want_color);
                Self::open_source_element(
                    &format!("v4l2src device={node}"),
                    camera_source,
                    force_ir_yuy2,
                    None,
                )
            }
            Err(err) => Err(err),
        }
    }

    /// A live source reaches `Playing` asynchronously, so a device rejected as busy fails after
    /// `set_state` returned success. Letting the change settle turns that into an open failure.
    fn start_pipeline(pipeline: &gstreamer::Pipeline, camera_source: &str) -> anyhow::Result<()> {
        let settled =
            pipeline
                .set_state(gstreamer::State::Playing)
                .and_then(|change| match change {
                    gstreamer::StateChangeSuccess::Async => {
                        // A timeout is not a failure: a slow camera may still be starting, and the
                        // frame loop reports an error that arrives later.
                        pipeline
                            .state(gstreamer::ClockTime::from_mseconds(
                                PIPELINE_START_TIMEOUT_MS,
                            ))
                            .0
                            .map(|_| change)
                    }
                    other => Ok(other),
                });

        let Err(e) = settled else {
            return Ok(());
        };

        let detail = bus_error_detail(pipeline);
        let _ = pipeline.set_state(gstreamer::State::Null);
        if let Some(detail) = detail {
            if device_is_busy(&detail) {
                anyhow::bail!(
                    "The camera is already in use by another program ({camera_source}): {detail}"
                );
            }
            anyhow::bail!("Failed to start pipeline for {camera_source}: {e} ({detail})");
        }
        anyhow::bail!("Failed to start pipeline for {camera_source}: {e}");
    }

    fn open_source_element(
        src_element: &str,
        camera_source: &str,
        force_ir_yuy2: bool,
        pipewire: Option<PipeWireSession>,
    ) -> anyhow::Result<Self> {
        let pipeline_str = camera_pipeline(src_element, force_ir_yuy2);
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

        Self::start_pipeline(&pipeline, camera_source)?;

        Ok(Self {
            pipeline,
            appsink,
            _pipewire: pipewire,
            stream_error: None,
            fps: Mutex::new(None),
        })
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

        let fraction = video_info.fps();
        let (num, denom) = (fraction.numer(), fraction.denom());
        if num > 0 && denom > 0 {
            let fps_val = num as f64 / denom as f64;
            if let Ok(mut guard) = self.fps.lock()
                && guard.is_none()
            {
                *guard = Some(fps_val);
            }
        }

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

    fn poll_frame(&mut self, timeout: gstreamer::ClockTime) -> FramePoll {
        if let Some(sample) = self.appsink.try_pull_sample(timeout) {
            return match self.sample_to_mat(&sample) {
                Ok(mat) => FramePoll::Frame(mat),
                Err(err) => {
                    warn!("Dropping camera frame: {err:#}");
                    FramePoll::Timeout
                }
            };
        }

        // A failing element also pushes EOS, so the sink's flag would hide the reason. Drain
        // rather than filter-pop: a zero timeout gives up on the first message that misses.
        if let Some(bus) = self.pipeline.bus() {
            while let Some(msg) = bus.pop() {
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
                        let detail =
                            format!("{}: {}", err.error(), err.debug().unwrap_or_default());
                        self.stream_error = Some(if device_is_busy(&detail) {
                            format!("The camera is already in use by another program: {detail}")
                        } else {
                            detail
                        });
                        return FramePoll::Ended;
                    }
                    gstreamer::MessageView::Eos(_) => {
                        info!("Camera stream ended (EOS)");
                        return FramePoll::Ended;
                    }
                    _ => {}
                }
            }
        }
        if self.appsink.is_eos() {
            info!("Camera stream ended (EOS)");
            return FramePoll::Ended;
        }
        let (_, current_state, _) = self.pipeline.state(Some(gstreamer::ClockTime::ZERO));
        if current_state != gstreamer::State::Playing && current_state != gstreamer::State::Paused {
            info!("Camera pipeline stopped: {:?}", current_state);
            return FramePoll::Ended;
        }

        FramePoll::Timeout
    }

    pub fn take_stream_error(&mut self) -> Option<String> {
        self.stream_error.take()
    }

    pub fn fps(&self) -> f64 {
        if let Ok(guard) = self.fps.lock()
            && let Some(fps) = *guard
            && fps > 0.0
            && fps.is_finite()
        {
            return fps;
        }
        if let Some(caps) = self
            .appsink
            .static_pad("sink")
            .and_then(|p| p.current_caps())
            && let Ok(video_info) = gstreamer_video::VideoInfo::from_caps(&caps)
        {
            let fraction = video_info.fps();
            let (num, denom) = (fraction.numer(), fraction.denom());
            if num > 0 && denom > 0 {
                let fps_val = num as f64 / denom as f64;
                if let Ok(mut guard) = self.fps.lock() {
                    *guard = Some(fps_val);
                }
                return fps_val;
            }
        }
        crate::config::DEFAULT_CAMERA_FPS
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

fn camera_pipeline(src_element: &str, force_ir_yuy2: bool) -> String {
    if force_ir_yuy2 {
        // Dell/Realtek single-node RGB/IR modules silently stay in RGB mode unless the
        // stream is negotiated as uncompressed YUY2 at this exact resolution.
        format!(
            "{src_element} ! video/x-raw,format=YUY2,width={REALTEK_IR_YUY2_WIDTH},height={REALTEK_IR_YUY2_HEIGHT},pixel-aspect-ratio=1/1 ! videoconvert ! videoscale ! appsink name=gaze_sink"
        )
    } else {
        format!(
            "{src_element} ! video/x-raw,pixel-aspect-ratio=1/1; image/jpeg ! decodebin ! videoconvert ! videoscale ! appsink name=gaze_sink"
        )
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

/// The IR picker, always led by an explicit "None" entry so a list index means
/// the same thing in every front end.
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
struct CameraEntry {
    display_name: String,
    target: String,
    node: Option<String>,
}

fn label_camera_entries(entries: Vec<CameraEntry>) -> Vec<(String, String)> {
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

fn collect_camera_entries(want_color: Option<bool>) -> anyhow::Result<Vec<CameraEntry>> {
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
            let target = format!("pipewiresrc target-object={}", target);
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

fn v4l2_node_of(props: &gstreamer::StructureRef) -> Option<String> {
    if let Some(path) = string_property(props, "api.v4l2.path") {
        return Some(path);
    }
    string_property(props, "device.path").filter(|path| path.starts_with("/dev/video"))
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
    fn pipewire_elements_take_the_bound_descriptor() {
        assert_eq!(bind_pipewire_fd("pipewiresrc", 7), "pipewiresrc fd=7");
        assert_eq!(
            bind_pipewire_fd("pipewiresrc target-object=cam1", 7),
            "pipewiresrc target-object=cam1 fd=7"
        );
    }

    #[test]
    fn a_caller_supplied_descriptor_is_not_doubled() {
        assert_eq!(bind_pipewire_fd("pipewiresrc fd=5", 7), "pipewiresrc fd=5");
    }

    #[test]
    fn a_held_device_is_named_as_busy() {
        assert!(device_is_busy(
            "Could not open device: Device or resource busy"
        ));
        assert!(device_is_busy("v4l2 returned EBUSY"));
        assert!(!device_is_busy("No such file or directory"));
    }

    #[test]
    fn non_pipewire_elements_are_left_alone() {
        // A V4L2 node has no PipeWire connection to inherit, and `fd=` means something else
        // entirely on other elements.
        assert_eq!(
            bind_pipewire_fd("v4l2src device=/dev/video0", 7),
            "v4l2src device=/dev/video0"
        );
        assert_eq!(bind_pipewire_fd("videotestsrc", 7), "videotestsrc");
        // Guard against matching an element that merely starts with the same letters.
        assert_eq!(bind_pipewire_fd("pipewiresrcfoo", 7), "pipewiresrcfoo");
    }

    #[test]
    fn a_missing_socket_is_reported_rather_than_bound() {
        assert!(PipeWireSession::connect_for_uid(u32::MAX).is_err());
    }

    #[test]
    fn only_an_untargeted_pipewire_source_falls_back_to_any_camera() {
        assert_eq!(v4l2_fallback_for("pipewiresrc"), V4l2Fallback::AnyDevice);
        assert_eq!(
            v4l2_fallback_for("v4l2src device=/dev/video0"),
            V4l2Fallback::None
        );
        assert_eq!(v4l2_fallback_for("pipewiresrcfoo"), V4l2Fallback::None);
        // A PipeWire source with no target still means "any camera", but it is not the bare
        // element, so it takes the field-parsing path.
        assert_eq!(v4l2_fallback_for("pipewiresrc fd=7"), V4l2Fallback::None);
    }

    #[test]
    fn a_pinned_target_falls_back_only_to_its_own_node() {
        assert_eq!(
            v4l2_fallback_for("pipewiresrc target-object=cam1"),
            V4l2Fallback::SameNode("cam1".to_string())
        );
        // The quoting and the trailing fields belong to the element, not to the target name.
        assert_eq!(
            v4l2_fallback_for("pipewiresrc target-object=\"cam1\" fd=7"),
            V4l2Fallback::SameNode("cam1".to_string())
        );
        assert_eq!(
            v4l2_fallback_for("pipewiresrc target-object=v4l2_input.pci-0000_00_14.0-usb-0_7_1.0"),
            V4l2Fallback::SameNode("v4l2_input.pci-0000_00_14.0-usb-0_7_1.0".to_string())
        );
        assert_eq!(
            v4l2_fallback_for("pipewiresrc target-object="),
            V4l2Fallback::None
        );
    }

    #[test]
    fn a_pipewire_target_names_the_udev_path_of_its_node() {
        assert_eq!(
            v4l2_by_path_for_target("v4l2_input.pci-0000_00_14.0-usb-0_7_1.0").as_deref(),
            Some("pci-0000:00:14.0-usb-0:7:1.0")
        );
        // Only V4L2 nodes are linked by path; anything else has no node to fall back to.
        assert_eq!(v4l2_by_path_for_target("alsa_input.pci-0000_00_1f.3"), None);
        assert_eq!(v4l2_by_path_for_target("51"), None);
        assert_eq!(v4l2_by_path_for_target("v4l2_input."), None);
    }

    #[test]
    fn a_failing_stream_keeps_the_reason_it_ended() {
        // A failing element pushes EOS as well as posting the error, so the reason survives
        // only if the bus is read first.
        let mut camera =
            Camera::open("videotestsrc ! identity error-after=3").expect("identity pipeline");
        let stop = AtomicBool::new(false);
        while camera.next_interruptible(&stop).is_some() {}

        let reason = camera
            .take_stream_error()
            .expect("a pipeline error must be kept");
        assert!(
            reason.contains("Failed after iterations as requested"),
            "unexpected reason: {reason}"
        );
        assert!(
            camera.take_stream_error().is_none(),
            "the reason is taken, so it is reported once"
        );
    }

    #[test]
    fn a_clean_end_of_stream_leaves_no_reason() {
        let mut camera = Camera::open("videotestsrc num-buffers=2").expect("videotestsrc pipeline");
        let stop = AtomicBool::new(false);
        while camera.next_interruptible(&stop).is_some() {}

        assert!(
            camera.take_stream_error().is_none(),
            "an ordinary EOS is not a failure"
        );
    }

    #[test]
    fn a_pinned_thread_keeps_its_session_when_the_default_moves() {
        // A capture thread outlives the claim that started it, and the next claim rebinds the
        // process-wide default. What the thread opens must not follow it.
        let (tx, rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let worker = std::thread::spawn(move || {
            bind_pipewire_uid_for_thread(Some(1000));
            tx.send(current_pipewire_uid()).unwrap();
            go_rx.recv().unwrap();
            tx.send(current_pipewire_uid()).unwrap();
        });

        assert_eq!(rx.recv().unwrap(), Some(1000));
        set_pipewire_uid(Some(1001));
        go_tx.send(()).unwrap();
        assert_eq!(rx.recv().unwrap(), Some(1000));
        worker.join().unwrap();

        // An unpinned thread still follows the default, which is what a claim sets it for.
        assert_eq!(
            std::thread::spawn(current_pipewire_uid).join().unwrap(),
            Some(1001)
        );
        set_pipewire_uid(None);
    }

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
    fn realtek_ir_pipeline_forces_required_uncompressed_mode() {
        let pipeline = camera_pipeline("v4l2src device=/dev/video0", true);
        assert!(pipeline.contains("video/x-raw,format=YUY2,width=640,height=480"));
        assert!(!pipeline.contains("image/jpeg"));

        let rgb_pipeline = camera_pipeline("v4l2src device=/dev/video0", false);
        assert!(rgb_pipeline.contains("image/jpeg"));
    }

    #[test]
    fn forced_ir_yuy2_follows_the_device_profile_flag() {
        // Both single-node Realtek modules are flagged in ir-profiles/*.toml.
        assert!(requires_forced_ir_yuy2(0x0bda, 0x5767, false));
        assert!(requires_forced_ir_yuy2(0x0bda, 0x58c2, false));

        // The quirk is IR-only, and never applies to unflagged or unknown devices.
        assert!(!requires_forced_ir_yuy2(0x0bda, 0x58c2, true));
        assert!(!requires_forced_ir_yuy2(0x046d, 0x085e, false));
        assert!(!requires_forced_ir_yuy2(0xdead, 0xbeef, false));
    }

    #[test]
    fn configured_sources_leave_ir_empty_when_no_ir_configured() {
        let cameras = CameraConfig {
            rgb: "primary".to_string(),
            ir: String::new(),
            emitter_enabled: false,
            dark_luma_threshold: 30,
            parallel_capture: "never".to_string(),
        };
        let sources = resolve_configured_sources(&cameras);
        assert_eq!(sources.rgb, "primary");
        assert_eq!(sources.ir, "");
        assert_eq!(sources.ir_node, "");
        assert_eq!(
            preferred_capture_source(&cameras),
            ("primary".to_string(), false)
        );
    }

    fn cameras_with(rgb: &str, ir: &str) -> CameraConfig {
        CameraConfig {
            rgb: rgb.to_string(),
            ir: ir.to_string(),
            emitter_enabled: false,
            dark_luma_threshold: 30,
            parallel_capture: "never".to_string(),
        }
    }

    #[test]
    fn two_nodes_on_one_hardware_function_must_capture_one_at_a_time() {
        assert!(functions_must_serialize(
            Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0".to_string()),
            Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0".to_string()),
        ));
    }

    #[test]
    fn separate_hardware_functions_may_stream_concurrently() {
        assert!(!functions_must_serialize(
            Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0".to_string()),
            Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.2".to_string()),
        ));
    }

    #[test]
    fn an_unreadable_hardware_function_keeps_the_serial_path() {
        let known = || Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0".to_string());
        assert!(functions_must_serialize(None, known()));
        assert!(functions_must_serialize(known(), None));
        assert!(functions_must_serialize(None, None));
    }

    #[test]
    fn a_single_node_rgb_ir_module_never_captures_in_parallel() {
        assert!(capture_must_serialize(
            Some("/dev/video0"),
            Some("/dev/video0")
        ));
    }

    #[test]
    fn an_unresolvable_capture_node_falls_back_to_serial() {
        assert!(capture_must_serialize(Some("/dev/video0"), None));
        assert!(capture_must_serialize(None, None));
        assert!(capture_must_serialize(
            Some("/dev/video-absent-rgb"),
            Some("/dev/video-absent-ir")
        ));
    }

    #[test]
    fn a_primary_rgb_source_serializes_against_a_dual_sensor_ir_function() {
        let ir_function = "/sys/devices/pci0000:00/usb1/1-5/1-5:1.0";
        assert!(serialize_for_unresolved_rgb(
            &[ir_function.to_string()],
            Some(ir_function)
        ));
    }

    #[test]
    fn a_primary_rgb_source_streams_beside_an_ir_only_function() {
        assert!(!serialize_for_unresolved_rgb(
            &["/sys/devices/pci0000:00/usb1/1-3/1-3:1.0".to_string()],
            Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0")
        ));
    }

    #[test]
    fn a_primary_rgb_source_stays_serial_when_nothing_enumerates() {
        let ir_function = "/sys/devices/pci0000:00/usb1/1-5/1-5:1.0";
        assert!(serialize_for_unresolved_rgb(&[], Some(ir_function)));
        assert!(serialize_for_unresolved_rgb(
            &[ir_function.to_string()],
            None
        ));
        assert!(serialize_for_unresolved_rgb(&[], None));
    }

    #[test]
    fn always_and_never_skip_hardware_detection_entirely() {
        let mut cameras = cameras_with("primary", "/dev/video2");

        cameras.parallel_capture = "never".to_string();
        assert!(resolve_configured_sources(&cameras).serial_capture);

        cameras.parallel_capture = "always".to_string();
        assert!(!resolve_configured_sources(&cameras).serial_capture);
    }

    #[test]
    fn an_unrecognised_capture_mode_stays_serial() {
        let mut cameras = cameras_with("primary", "/dev/video2");
        cameras.parallel_capture = "sometimes".to_string();
        assert!(resolve_configured_sources(&cameras).serial_capture);
    }

    #[test]
    fn a_pipewire_camera_can_back_two_previews_at_once() {
        assert!(preview_can_be_shared(&cameras_with("primary", "")));
        assert!(preview_can_be_shared(&cameras_with(
            "pipewiresrc target-object=v4l2_input.pci-0000:00:14.0-usb-0:5:1.0",
            ""
        )));
    }

    #[test]
    fn a_v4l2_camera_cannot_be_streamed_twice() {
        assert!(!preview_can_be_shared(&cameras_with("/dev/video0", "")));
        assert!(!preview_can_be_shared(&cameras_with("usb:04f2:b6d9", "")));
    }

    #[test]
    fn a_dual_spectrum_setup_needs_the_rgb_camera_released_between_steps() {
        assert!(!preview_can_be_shared(&cameras_with(
            "primary",
            "/dev/video2"
        )));
    }

    #[test]
    fn an_ir_only_setup_is_not_shareable() {
        assert!(!preview_can_be_shared(&cameras_with("", "/dev/video2")));
    }

    #[test]
    fn configured_sources_resolve_an_ir_device_node() {
        let cameras = CameraConfig {
            rgb: "primary".to_string(),
            ir: "/dev/video2".to_string(),
            emitter_enabled: true,
            dark_luma_threshold: 30,
            parallel_capture: "never".to_string(),
        };
        let sources = resolve_configured_sources(&cameras);
        assert_eq!(sources.rgb, "primary");
        assert_eq!(sources.ir, "/dev/video2");
        assert_eq!(sources.ir_node, "/dev/video2");
    }

    #[test]
    fn preferred_capture_source_falls_back_to_ir_when_rgb_is_unset() {
        let cameras = CameraConfig {
            rgb: String::new(),
            ir: "/dev/video2".to_string(),
            emitter_enabled: true,
            dark_luma_threshold: 30,
            parallel_capture: "never".to_string(),
        };
        assert_eq!(
            preferred_capture_source(&cameras),
            ("/dev/video2".to_string(), true)
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
    fn select_usb_node_reuses_single_node_for_known_ir_profile() {
        let nodes = vec![VideoNodeInfo {
            node: "/dev/video6".to_string(),
            vid: 0x0bda,
            pid: 0x58c2,
            is_color: true,
        }];

        assert_eq!(
            select_usb_node(&nodes, 0x0bda, 0x58c2, false),
            Some("/dev/video6".to_string())
        );
    }

    #[test]
    fn configured_sources_leave_the_node_empty_for_a_pipewire_ir_source() {
        let cameras = CameraConfig {
            rgb: "primary".to_string(),
            ir: "pipewiresrc target-object=device-name".to_string(),
            emitter_enabled: true,
            dark_luma_threshold: 30,
            parallel_capture: "auto".to_string(),
        };
        let sources = resolve_configured_sources(&cameras);
        assert_eq!(sources.ir, "pipewiresrc target-object=device-name");
        assert_eq!(sources.ir_node, "");
        assert!(
            sources.serial_capture,
            "an unresolvable node must keep the safe serial capture path"
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
