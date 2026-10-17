// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::os::fd::AsRawFd;

use gaze_core::config::{CameraConfig, DEFAULT_RGB_CAMERA};
use gaze_core::ir::devices::{camera_function_of, find_device, usb_ids_of};

use super::*;

pub(super) fn requires_forced_ir_yuy2(vid: u16, pid: u16, want_color: bool) -> bool {
    !want_color && find_device(vid, pid).is_some_and(|device| device.requires_ir_yuy2)
}

/// Looks up this quirk by the resolved USB device, not by the source string. This lets
/// `usb:VVVV:PPPP`, `/dev/videoN`, and PipeWire sources all use the same device profile.
pub(super) fn node_requires_forced_ir_yuy2(node: &str, want_color: bool) -> bool {
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

pub(super) fn non_empty(value: &str) -> Option<&str> {
    (!value.is_empty()).then_some(value)
}

pub(super) fn capture_must_serialize(rgb_node: Option<&str>, ir_node: Option<&str>) -> bool {
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

pub(super) fn functions_must_serialize(
    rgb_function: Option<String>,
    ir_function: Option<String>,
) -> bool {
    // Different video nodes can share one UVC function and cannot necessarily stream together.
    // Unknown identities must serialize too; only proven distinct functions run concurrently.
    match (rgb_function, ir_function) {
        (Some(rgb), Some(ir)) => rgb == ir,
        _ => true,
    }
}

pub(super) fn color_camera_functions() -> Vec<String> {
    v4l2_nodes_with_color(true)
        .iter()
        .filter_map(|node| camera_function_of(node))
        .collect()
}

pub(super) fn serialize_for_unresolved_rgb(
    color_functions: &[String],
    ir_function: Option<&str>,
) -> bool {
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
pub(super) enum SourceElement {
    Element(String),
    ResolveUsb {
        vid: u16,
        pid: u16,
        want_color: bool,
    },
}

/// Turn a configured `rgb`/`ir` value into a GStreamer source element. `primary` needs a PipeWire
/// session; node and `usb:` specs use `v4l2src`, which still works in greeters that have none.
pub(super) fn classify_source(source: &str, want_color: bool) -> anyhow::Result<SourceElement> {
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
pub(super) struct VideoNodeInfo {
    pub(super) node: String,
    pub(super) vid: u16,
    pub(super) pid: u16,
    pub(super) is_color: bool,
}

/// Pick the `/dev/video<n>` node for `vid:pid` with the requested color-ness, lowest-numbered
/// first so the choice stays stable across boots.
pub(super) fn select_usb_node(
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

pub(super) fn video_node_index(node: &str) -> Option<u32> {
    node.strip_prefix("/dev/video")?.parse().ok()
}

/// Scan V4L2 nodes for one matching `vid:pid` with the requested color-ness. The GStreamer device
/// monitor enumerates via the plain V4L2 provider without a PipeWire session; ids come from sysfs.
pub(super) fn resolve_usb_video_node(vid: u16, pid: u16, want_color: bool) -> Option<String> {
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

pub(super) fn first_v4l2_node(want_color: bool) -> Option<String> {
    v4l2_nodes_with_color(want_color).into_iter().next()
}

/// Every V4L2 node with the requested color-ness, lowest-numbered first. Enumeration goes through
/// the plain V4L2 provider, so it still answers in a greeter that has no PipeWire session.
pub(super) fn v4l2_nodes_with_color(want_color: bool) -> Vec<String> {
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

pub(super) fn device_video_node(device: &gstreamer::Device) -> Option<String> {
    let props = device.properties()?;
    if let Some(path) = string_property(&props, "api.v4l2.path")
        && path.starts_with("/dev/video")
    {
        return Some(path);
    }
    let path = string_property(&props, "device.path")?;
    path.starts_with("/dev/video").then_some(path)
}

pub fn resolve_privileged_node(source: &str, want_color: bool) -> Option<String> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }
    if source == DEFAULT_RGB_CAMERA {
        return first_v4l2_node(want_color);
    }
    if let Some((vid, pid)) = parse_usb_spec(source) {
        return resolve_usb_video_node(vid, pid, want_color);
    }
    if source.starts_with("/dev/video") {
        let is_node = source
            .strip_prefix("/dev/video")
            .is_some_and(|index| !index.is_empty() && index.chars().all(|c| c.is_ascii_digit()));
        return is_node.then(|| source.to_string());
    }
    match v4l2_fallback_for(source) {
        V4l2Fallback::AnyDevice => first_v4l2_node(want_color),
        V4l2Fallback::SameNode(target) => node_from_pipewire_target(&target, want_color),
        V4l2Fallback::None => None,
    }
}

pub(super) fn libcamera_fallback_applies(
    camera_source: &str,
    want_color: bool,
    failed_node: Option<&str>,
) -> bool {
    want_color
        && camera_source.trim() == DEFAULT_RGB_CAMERA
        && failed_node.is_none_or(node_requires_media_controller)
}

pub(super) const VIDIOC_QUERYCAP: libc::c_ulong = 0x8068_5600;

pub(super) const V4L2_CAP_IO_MC: u32 = 0x2000_0000;

pub(super) const V4L2_CAP_DEVICE_CAPS: u32 = 0x8000_0000;

#[repr(C)]
pub(super) struct V4l2Capability {
    pub(super) _driver: [u8; 16],
    pub(super) _card: [u8; 32],
    pub(super) _bus_info: [u8; 32],
    pub(super) _version: u32,
    pub(super) capabilities: u32,
    pub(super) device_caps: u32,
    pub(super) _reserved: [u32; 3],
}

pub(super) fn effective_device_caps(capabilities: u32, device_caps: u32) -> u32 {
    if capabilities & V4L2_CAP_DEVICE_CAPS != 0 {
        device_caps
    } else {
        capabilities
    }
}

pub(super) fn node_requires_media_controller(node: &str) -> bool {
    use std::os::unix::fs::OpenOptionsExt;

    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(node)
    else {
        return false;
    };
    let mut cap = V4l2Capability {
        _driver: [0; 16],
        _card: [0; 32],
        _bus_info: [0; 32],
        _version: 0,
        capabilities: 0,
        device_caps: 0,
        _reserved: [0; 3],
    };
    let ret = unsafe {
        libc::ioctl(
            file.as_raw_fd(),
            VIDIOC_QUERYCAP,
            &mut cap as *mut V4l2Capability,
        )
    };
    ret == 0 && effective_device_caps(cap.capabilities, cap.device_caps) & V4L2_CAP_IO_MC != 0
}

pub(super) fn libcamera_color_source() -> Option<String> {
    gstreamer::ElementFactory::find("libcamerasrc")?;
    let monitor = gstreamer::DeviceMonitor::new();
    monitor.add_filter(Some("Video/Source"), None);
    monitor.start().ok()?;
    wait_for_device_updates(&monitor);
    let devices = monitor.devices();
    monitor.stop();

    devices
        .iter()
        .filter(|device| has_color_caps(device))
        .find_map(|device| {
            let element = device.create_element(None).ok()?;
            let factory = element.factory()?;
            if factory.name().as_str() != "libcamerasrc" {
                return None;
            }
            let name = element.property::<Option<String>>("camera-name")?;
            Some(libcamera_source_element(&name))
        })
}

/// libcamera logs the cause to stderr, not the bus. When its software ISP cannot start its IPA,
/// it offers only raw Bayer, which the pipeline cannot take.
pub(super) fn libcamera_negotiation_hint(detail: &str) -> Option<&'static str> {
    detail.contains("not-negotiated").then_some(
        ". libcamera offered no processed format, usually because its software ISP failed to \
         start; if `journalctl -u gazed` shows `Failed to fork`, update libcamera \
         (see https://gaze.gundulabs.com/guide/troubleshooting)",
    )
}

pub(super) fn libcamera_source_element(camera_name: &str) -> String {
    let escaped = camera_name.replace('\\', "\\\\").replace('"', "\\\"");
    format!("libcamerasrc camera-name=\"{escaped}\"")
}

pub(super) const V4L2_BY_PATH_DIR: &str = "/dev/v4l/by-path";

/// PipeWire names a V4L2 camera `v4l2_input.<udev ID_PATH>` with `:` rewritten as `_`, and udev
/// links that path under `/dev/v4l/by-path`, so a pinned target still resolves with no session.
pub(super) fn v4l2_by_path_for_target(target: &str) -> Option<String> {
    let by_path = target.strip_prefix("v4l2_input.")?;
    (!by_path.is_empty()).then(|| by_path.replace('_', ":"))
}

pub(super) fn without_dedup_suffix(by_path: &str) -> Option<&str> {
    let (base, counter) = by_path.rsplit_once('.')?;
    (!base.is_empty() && !counter.is_empty() && counter.chars().all(|c| c.is_ascii_digit()))
        .then_some(base)
}

/// The nodes udev links for one `by-path`, lowest `video-index` first.
pub(super) fn nodes_for_by_path(by_path: &str) -> Vec<String> {
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

pub(super) fn pipewire_target_nodes(target: &str) -> Option<Vec<String>> {
    let by_path = v4l2_by_path_for_target(target)?;
    let mut nodes = nodes_for_by_path(&by_path);
    if nodes.is_empty()
        && let Some(base) = without_dedup_suffix(&by_path)
    {
        nodes = nodes_for_by_path(base);
    }
    Some(nodes)
}

pub fn camera_disconnected(source: &str, want_color: bool) -> bool {
    let source = source.trim();
    if let V4l2Fallback::SameNode(target) = v4l2_fallback_for(source) {
        return pipewire_target_nodes(&target).is_some_and(|nodes| nodes.is_empty());
    }
    if source.starts_with("/dev/video") {
        return resolve_privileged_node(source, want_color)
            .is_some_and(|node| !std::path::Path::new(&node).exists());
    }
    let names_device =
        source == DEFAULT_RGB_CAMERA || source == "pipewiresrc" || parse_usb_spec(source).is_some();
    names_device && resolve_privileged_node(source, want_color).is_none()
}

/// Resolves a pinned PipeWire target to its V4L2 node. A device may link several nodes, including
/// non-capture nodes such as metadata; negotiated caps distinguish the usable camera node.
pub(super) fn node_from_pipewire_target(target: &str, want_color: bool) -> Option<String> {
    let nodes = pipewire_target_nodes(target)?;
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
