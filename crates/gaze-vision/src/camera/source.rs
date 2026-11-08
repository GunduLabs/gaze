// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt};

use gaze_core::config::{BridgeSource, CameraConfig, CameraSources, DEFAULT_RGB_CAMERA};
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
    pub rgb: CameraSources,
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

pub fn resolve_rgb_source(cameras: &CameraConfig) -> Option<CameraSources> {
    (!cameras.rgb.is_empty()).then(|| cameras.rgb.clone())
}

pub fn resolve_configured_sources(cameras: &CameraConfig) -> ConfiguredCameraSources {
    let rgb = cameras.rgb.clone();
    let (ir, ir_node) = resolve_ir_source(cameras).unwrap_or_default();
    let serial_capture = match cameras.parallel_capture() {
        "always" => false,
        "auto" => {
            // Any fallback may be selected when capture opens. Only run in
            // parallel if every candidate is proven independent of IR.
            rgb.is_empty()
                || rgb.iter().any(|source| {
                    let rgb_node = resolve_node_for(source, true);
                    capture_must_serialize(rgb_node.as_deref(), non_empty(&ir_node))
                })
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

pub fn preferred_capture_source(cameras: &CameraConfig) -> (CameraSources, bool) {
    if let Some(rgb_source) = resolve_rgb_source(cameras) {
        (rgb_source, false)
    } else if let Some((ir_source, _)) = resolve_ir_source(cameras) {
        (ir_source.into(), true)
    } else {
        (DEFAULT_RGB_CAMERA.into(), false)
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
            "camera source cannot be empty; use \"primary\", \"/dev/video<n>\", \"usb:VVVV:PPPP\", \"libcamera:<name>\", or a GStreamer source"
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
    if source.starts_with(LIBCAMERA_SOURCE_PREFIX) {
        let Some(name) = parse_libcamera_spec(source) else {
            anyhow::bail!(
                "invalid libcamera camera spec {source:?}; expected libcamera:<camera name>"
            );
        };
        return Ok(SourceElement::Element(libcamera_source_element(name)));
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
    // Probe the provider directly: a DeviceMonitor drops its devices whenever the libcamera
    // provider is present (it hides v4l2deviceprovider), leaving no node without PipeWire.
    let Some(provider) = gstreamer::DeviceProviderFactory::by_name("v4l2deviceprovider") else {
        return Vec::new();
    };
    let devices = provider.devices();
    let raw_caps = gstreamer::Caps::builder("video/x-raw").build();

    let mut seen = std::collections::HashSet::new();
    let mut nodes: Vec<String> = devices
        .iter()
        .filter(|device| is_raw_video_source(device, &raw_caps))
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

fn is_raw_video_source(device: &gstreamer::Device, raw_caps: &gstreamer::Caps) -> bool {
    device.has_classes("Video/Source")
        && device
            .caps()
            .is_some_and(|caps| caps.can_intersect(raw_caps))
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

/// True when the node has a PCI or USB device ancestor. USB controllers can themselves
/// live on the platform bus, so the sysfs path need not start with a PCI root.
pub fn is_physical_node(node: &str) -> bool {
    if video_node_index(node).is_none() {
        return false;
    }
    let name = node.strip_prefix("/dev/").unwrap();
    std::fs::canonicalize(format!("/sys/class/video4linux/{name}/device"))
        .is_ok_and(|path| has_physical_bus_ancestor(&path))
}

fn has_physical_bus_ancestor(path: &std::path::Path) -> bool {
    path.ancestors().any(|ancestor| {
        std::fs::canonicalize(ancestor.join("subsystem")).is_ok_and(|bus| {
            matches!(
                bus.file_name().and_then(|name| name.to_str()),
                Some("pci" | "usb")
            )
        })
    })
}

/// Gates opening `node` for privileged capture. Physical nodes always pass. A virtual node
/// (for example a v4l2loopback fed by a userspace bridge for an IPU6 sensor) passes only when
/// `bridge` — the operator-declared `cameras.ir_source_marker`/`ir_source_driver` — proves
/// which physical camera feeds it.
pub(super) fn check_trusted_node(
    node: &str,
    bridge: Option<BridgeSource<'_>>,
) -> anyhow::Result<()> {
    if is_physical_node(node) {
        return Ok(());
    }
    let Some(bridge) = bridge else {
        anyhow::bail!("refusing {node}: not a physical PCI/USB camera (virtual or loopback?)")
    };
    let source =
        check_bridge_marker(&bridge).map_err(|err| anyhow::anyhow!("refusing {node}: {err:#}"))?;
    if node_grants_untrusted_write(node) {
        warn!(
            "trusted bridge node {node} is writable by unprivileged users; recorded footage \
             could be replayed into it — restrict the node (for example drop the uaccess tag \
             and use root:video 0660 permissions)"
        );
    }
    info!(
        "{node} is virtual; bridge marker {} vouches for physical source {source}",
        bridge.marker
    );
    Ok(())
}

/// Validates the provenance `bridge` declares for a virtual capture node: the marker must be a
/// root-owned regular file that only root can write, naming a physical `/dev/video*` node that
/// is bound to `bridge.source_driver` when a driver is pinned. Returns the source node.
pub fn check_bridge_marker(bridge: &BridgeSource<'_>) -> anyhow::Result<String> {
    let source = bridge_marker_source(bridge.marker)?;
    if !is_physical_node(&source) {
        anyhow::bail!(
            "bridge marker {} names {source}, which is not a physical camera",
            bridge.marker
        );
    }
    if let Some(expected) = bridge.source_driver {
        let actual = node_driver(&source);
        if actual.as_deref() != Some(expected) {
            anyhow::bail!(
                "bridge source {source} is bound to {}, expected {expected}",
                actual.as_deref().unwrap_or("no driver")
            );
        }
    }
    Ok(source)
}

/// Reads `marker` and returns the `/dev/video*` node it names. The marker is a trust anchor:
/// it must be a regular file owned by root that group and others cannot write, so only a
/// privileged bridge service can create it.
fn bridge_marker_source(marker: &str) -> anyhow::Result<String> {
    let metadata = std::fs::metadata(marker).map_err(|e| {
        anyhow::anyhow!("cannot stat bridge marker {marker}: {e}; is the bridge running?")
    })?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        anyhow::bail!(
            "bridge marker {marker} must be a regular file owned by root and writable only by \
             its owner"
        );
    }
    let content = std::fs::read_to_string(marker)
        .map_err(|e| anyhow::anyhow!("cannot read bridge marker {marker}: {e}"))?;
    let source = content.lines().next().unwrap_or_default().trim();
    if video_node_index(source).is_none() {
        anyhow::bail!("bridge marker {marker} names {source:?}, which is not a /dev/video* node");
    }
    if !std::fs::metadata(source).is_ok_and(|m| m.file_type().is_char_device()) {
        anyhow::bail!("bridge marker {marker} names {source}, which is not present");
    }
    Ok(source.to_string())
}

/// Driver bound to the node's device, read from the sysfs `driver` link.
fn node_driver(node: &str) -> Option<String> {
    let name = node.strip_prefix("/dev/")?;
    std::fs::read_link(format!("/sys/class/video4linux/{name}/device/driver"))
        .ok()
        .and_then(|link| {
            link.file_name()
                .and_then(|name| name.to_str().map(str::to_string))
        })
}

/// True when `node` can be written by processes other than root or members of its group — the
/// replay vector a trusted bridge node must not have: anyone able to feed the node could pass
/// recorded footage off as the real sensor.
pub fn node_grants_untrusted_write(node: &str) -> bool {
    std::fs::metadata(node)
        .is_ok_and(|metadata| metadata.mode() & 0o002 != 0 || acl_grants_named_write(node))
}

/// True when the node's POSIX ACL grants write to a named user or group (for example the
/// `uaccess` entry udev adds for the seat user).
fn acl_grants_named_write(node: &str) -> bool {
    let Ok(path) = std::ffi::CString::new(node) else {
        return false;
    };
    let name = c"system.posix_acl_access".as_ptr();
    let size = unsafe { libc::getxattr(path.as_ptr(), name, std::ptr::null_mut(), 0) };
    if size <= 4 {
        return false;
    }
    let mut buffer = vec![0u8; size as usize];
    let read = unsafe {
        libc::getxattr(
            path.as_ptr(),
            name,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
        )
    };
    read > 4 && acl_xattr_grants_named_write(&buffer[..read as usize])
}

/// POSIX ACL xattr payload: a 4-byte header then 8-byte entries of tag, permissions, id.
/// Write to a named `ACL_USER` (0x02) or `ACL_GROUP` (0x08) entry grants write beyond the
/// owning user and group; named entries are limited by the `ACL_MASK` (0x10) entry.
fn acl_xattr_grants_named_write(xattr: &[u8]) -> bool {
    const ACL_USER: u16 = 0x02;
    const ACL_GROUP: u16 = 0x08;
    const ACL_MASK: u16 = 0x10;
    let Some(entries) = xattr.get(4..) else {
        return false;
    };
    let entries = entries.as_chunks::<8>().0;
    let mask = entries
        .iter()
        .find(|entry| u16::from_le_bytes([entry[0], entry[1]]) == ACL_MASK)
        .map(|entry| u16::from_le_bytes([entry[2], entry[3]]));
    entries.iter().any(|entry| {
        let tag = u16::from_le_bytes([entry[0], entry[1]]);
        let perm = u16::from_le_bytes([entry[2], entry[3]]);
        let effective = mask.map_or(perm, |mask| perm & mask);
        matches!(tag, ACL_USER | ACL_GROUP) && effective & 0x2 != 0
    })
}

fn select_physical_node(
    nodes: impl IntoIterator<Item = String>,
    mut is_physical: impl FnMut(&str) -> bool,
) -> Option<String> {
    nodes.into_iter().find(|node| is_physical(node))
}

fn first_physical_v4l2_node(want_color: bool) -> Option<String> {
    select_physical_node(v4l2_nodes_with_color(want_color), is_physical_node)
}

pub fn resolve_privileged_node(source: &str, want_color: bool) -> Option<String> {
    let source = source.trim();
    if source.is_empty() {
        return None;
    }
    if source == DEFAULT_RGB_CAMERA {
        return first_physical_v4l2_node(want_color);
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
        V4l2Fallback::AnyDevice => first_physical_v4l2_node(want_color),
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

pub(super) fn libcamera_camera_missing(detail: &str) -> bool {
    detail
        .to_ascii_lowercase()
        .contains("could not find a camera")
}

pub(super) fn libcamera_missing_hint(detail: &str) -> Option<&'static str> {
    libcamera_camera_missing(detail).then_some(
        ". libcamera does not list a camera while another program is using it, so close other \
         camera apps, check that the camera is connected, or pick a name from `cam --list`",
    )
}

fn libcamera_eventually_lists(mut lists: impl FnMut(&[String]) -> bool) -> bool {
    for attempt in 1..=PRIVILEGED_BUSY_ATTEMPTS {
        if lists(libcamera_camera_names().as_slice()) {
            return true;
        }
        if attempt < PRIVILEGED_BUSY_ATTEMPTS {
            std::thread::sleep(PRIVILEGED_BUSY_RETRY_DELAY);
        }
    }
    false
}

pub(super) fn libcamera_source_element(camera_name: &str) -> String {
    let escaped = camera_name.replace('\\', "\\\\").replace('"', "\\\"");
    format!("libcamerasrc camera-name=\"{escaped}\"")
}

/// Prefix that names a libcamera camera directly, e.g. `libcamera:\_SB_.PC00.LNK0` for an Intel
/// IPU6/IPU7 MIPI sensor whose `/dev/video*` nodes are raw capture nodes only libcamera can drive.
pub(super) const LIBCAMERA_SOURCE_PREFIX: &str = "libcamera:";

pub fn parse_libcamera_spec(source: &str) -> Option<&str> {
    source
        .trim()
        .strip_prefix(LIBCAMERA_SOURCE_PREFIX)
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

pub(super) const PIPEWIRE_LIBCAMERA_NODE_PREFIX: &str = "libcamera_input.";

pub(super) fn pipewire_libcamera_target(source: &str) -> Option<String> {
    let V4l2Fallback::SameNode(target) = v4l2_fallback_for(source.trim()) else {
        return None;
    };
    target
        .starts_with(PIPEWIRE_LIBCAMERA_NODE_PREFIX)
        .then_some(target)
}

/// Mirrors WirePlumber's Lua `gsub`, which turns each byte outside `[A-Za-z0-9_.-]` into `_`.
pub(super) fn wireplumber_sanitize(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.') {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

pub(super) fn libcamera_camera_for_target<'a>(
    target: &str,
    cameras: &'a [String],
) -> Option<&'a str> {
    let id = target.strip_prefix(PIPEWIRE_LIBCAMERA_NODE_PREFIX)?;
    let find = |id: &str| {
        cameras
            .iter()
            .find(|camera| wireplumber_sanitize(camera) == id)
            .map(String::as_str)
    };
    find(id).or_else(|| without_dedup_suffix(id).and_then(find))
}

/// Accepts the `libcamera_input.<id>` target the GUI saves as well as `libcamera:<name>`. The
/// daemon opens that camera with its own libcamera and never reads from the PipeWire session.
pub(super) fn libcamera_camera_for_source(source: &str) -> Option<String> {
    if let Some(name) = parse_libcamera_spec(source) {
        return Some(name.to_string());
    }
    let target = pipewire_libcamera_target(source)?;
    libcamera_camera_for_target(&target, &libcamera_camera_names()).map(str::to_string)
}

pub fn libcamera_camera_names() -> Vec<String> {
    if gstreamer::init().is_err() {
        return Vec::new();
    }
    // Probe the provider directly: in a session with PipeWire, a DeviceMonitor hides
    // libcameraprovider behind pipewiredeviceprovider and reports only pipewiresrc devices.
    let Some(provider) = gstreamer::DeviceProviderFactory::by_name("libcameraprovider") else {
        return Vec::new();
    };
    provider
        .devices()
        .iter()
        .filter_map(|device| {
            let element = device.create_element(None).ok()?;
            element.property::<Option<String>>("camera-name")
        })
        .collect()
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
    if let Some(name) = parse_libcamera_spec(source) {
        return !libcamera_eventually_lists(|cameras| cameras.iter().any(|camera| camera == name));
    }
    if let Some(target) = pipewire_libcamera_target(source) {
        return !libcamera_eventually_lists(|cameras| {
            libcamera_camera_for_target(&target, cameras).is_some()
        });
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    mod test_device {
        use gstreamer::{glib, subclass::prelude::*};

        #[derive(Default)]
        pub struct TestDevice;

        #[glib::object_subclass]
        impl ObjectSubclass for TestDevice {
            const NAME: &'static str = "GazeSourceFilterTestDevice";
            type Type = super::TestDevice;
            type ParentType = gstreamer::Device;
        }

        impl ObjectImpl for TestDevice {}
        impl GstObjectImpl for TestDevice {}
        impl DeviceImpl for TestDevice {}
    }

    gstreamer::glib::wrapper! {
        pub struct TestDevice(ObjectSubclass<test_device::TestDevice>)
            @extends gstreamer::Device, gstreamer::Object;
    }

    #[test]
    fn direct_provider_filter_accepts_only_raw_video_sources() {
        gstreamer::init().unwrap();
        let raw_caps = gstreamer::Caps::builder("video/x-raw").build();
        for (class, caps, expected) in [
            ("Video/Source", "video/x-raw,format=YUY2", true),
            ("Video/Source", "video/x-raw,format=GRAY8", true),
            ("Video/Sink", "video/x-raw,format=YUY2", false),
            ("Audio/Source", "video/x-raw", false),
            ("Video/Source", "image/jpeg", false),
            ("Video/Source", "image/jpeg;video/x-raw", true),
        ] {
            let caps: gstreamer::Caps = caps.parse().unwrap();
            let device: TestDevice = gstreamer::glib::Object::builder()
                .property("device-class", class)
                .property("caps", caps)
                .build();
            assert_eq!(
                is_raw_video_source(device.upcast_ref(), &raw_caps),
                expected
            );
        }
    }

    #[test]
    fn physical_detection_follows_bus_links_instead_of_path_prefixes() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "gaze-physical-camera-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for bus in ["pci", "usb", "platform"] {
            std::fs::create_dir_all(root.join("bus").join(bus)).unwrap();
        }
        for (device, bus, expected) in [
            ("devices/pci0000:00/0000:00:14.0", "pci", true),
            ("devices/platform/soc/usb1/1-1/1-1:1.0", "usb", true),
            ("devices/platform/vivid.0", "platform", false),
        ] {
            let device = root.join(device);
            std::fs::create_dir_all(device.join("video4linux/video0")).unwrap();
            symlink(root.join("bus").join(bus), device.join("subsystem")).unwrap();
            assert_eq!(has_physical_bus_ancestor(&device), expected);
            assert_eq!(
                has_physical_bus_ancestor(&device.join("video4linux/video0")),
                expected
            );
        }
        let virtual_device = root.join("devices/virtual/video4linux/video10");
        std::fs::create_dir_all(&virtual_device).unwrap();
        assert!(!has_physical_bus_ancestor(&virtual_device));
        assert!(!has_physical_bus_ancestor(&root.join("missing")));
        assert!(!is_physical_node("/dev/video-does-not-exist"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_selection_skips_virtual_nodes_and_preserves_order() {
        let nodes = || ["/dev/video0", "/dev/video2", "/dev/video4"].map(String::from);
        assert_eq!(
            select_physical_node(nodes(), |node| node != "/dev/video0"),
            Some("/dev/video2".to_string())
        );
        assert_eq!(select_physical_node(nodes(), |_| false), None);
        assert_eq!(select_physical_node(Vec::new(), |_| true), None);
    }

    #[test]
    fn libcamera_specs_name_a_camera() {
        assert_eq!(
            parse_libcamera_spec("libcamera:\\_SB_.PC00.LNK0"),
            Some("\\_SB_.PC00.LNK0")
        );
        assert_eq!(parse_libcamera_spec("  libcamera: cam1 "), Some("cam1"));
        assert_eq!(parse_libcamera_spec("libcamera:"), None);
        assert_eq!(parse_libcamera_spec("/dev/video0"), None);
    }

    #[test]
    fn pipewire_libcamera_targets_map_back_to_their_camera() {
        let cameras = vec![
            "\\_SB_.PC00.LNK0".to_string(),
            "\\_SB_.PC00.LNK1".to_string(),
        ];
        assert_eq!(
            libcamera_camera_for_target("libcamera_input.__SB_.PC00.LNK1", &cameras),
            Some("\\_SB_.PC00.LNK1")
        );
        assert_eq!(
            libcamera_camera_for_target("libcamera_input.nope", &cameras),
            None
        );
        assert_eq!(
            libcamera_camera_for_target("v4l2_input.pci-0000_00_14.0", &cameras),
            None
        );
        assert_eq!(
            libcamera_camera_for_target("libcamera_input.__SB_.PC00.LNK1.2", &cameras),
            Some("\\_SB_.PC00.LNK1")
        );
        assert_eq!(
            wireplumber_sanitize("/base/i2c@1:cam 0"),
            "_base_i2c_1_cam_0"
        );
        assert_eq!(wireplumber_sanitize("caméra"), "cam__ra");
    }

    #[test]
    fn a_camera_libcamera_stops_listing_counts_as_missing() {
        let detail = "Could not find a camera named '\\_SB_.LNK1'.: \
                      libcamera::CameraMananger::get() returned nullptr";
        assert!(libcamera_camera_missing(detail));
        assert!(libcamera_missing_hint(detail).is_some());
        assert!(libcamera_camera_missing(
            "Could not find a camera for \"pipewiresrc target-object=libcamera_input.x\" in libcamera"
        ));
        assert!(!libcamera_camera_missing(
            "Internal data stream error: not-negotiated"
        ));
        assert!(libcamera_missing_hint("not-negotiated").is_none());
    }

    #[test]
    fn only_pipewire_libcamera_targets_are_libcamera_targets() {
        assert_eq!(
            pipewire_libcamera_target("pipewiresrc target-object=libcamera_input.__SB_.LNK0"),
            Some("libcamera_input.__SB_.LNK0".to_string())
        );
        assert_eq!(
            pipewire_libcamera_target("pipewiresrc target-object=v4l2_input.pci-0000_00_14.0"),
            None
        );
        assert_eq!(pipewire_libcamera_target("libcamera:\\_SB_.LNK0"), None);
        assert_eq!(pipewire_libcamera_target("pipewiresrc"), None);
    }

    #[test]
    fn libcamera_specs_classify_as_libcamerasrc() {
        assert_eq!(
            classify_source("libcamera:\\_SB_.PC00.LNK0", false).unwrap(),
            SourceElement::Element("libcamerasrc camera-name=\"\\\\_SB_.PC00.LNK0\"".into())
        );
        assert!(classify_source("libcamera:", true).is_err());
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
    fn a_deduplicated_pipewire_target_falls_back_to_its_udev_path() {
        let by_path = v4l2_by_path_for_target("v4l2_input.pci-0000_0d_00.4-usb-0_1_1.0.2").unwrap();
        assert_eq!(by_path, "pci-0000:0d:00.4-usb-0:1:1.0.2");
        assert_eq!(
            without_dedup_suffix(&by_path),
            Some("pci-0000:0d:00.4-usb-0:1:1.0")
        );
        assert_eq!(without_dedup_suffix("platform-camera"), None);
        assert_eq!(without_dedup_suffix("pci-0000:0d:00.4-usb-0:1:1.x"), None);
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
            rgb: "primary".into(),
            ir: String::new(),
            ir_frame_width: None,
            ir_frame_height: None,
            ir_source_marker: None,
            ir_source_driver: None,
            emitter_enabled: false,
            dark_luma_threshold: 30,
            parallel_capture: "never".to_string(),
            auto_rotate: true,
        };
        let sources = resolve_configured_sources(&cameras);
        assert_eq!(sources.rgb, "primary".into());
        assert_eq!(sources.ir, "");
        assert_eq!(sources.ir_node, "");
        assert_eq!(
            preferred_capture_source(&cameras),
            ("primary".into(), false)
        );
    }

    #[test]
    fn configured_and_preview_sources_preserve_the_entire_rgb_fallback_list() {
        let mut cameras = cameras_with("primary", "");
        cameras.rgb = CameraSources::Ordered(vec!["usb:046d:081b".into(), "primary".into()]);
        assert_eq!(resolve_configured_sources(&cameras).rgb, cameras.rgb);
        assert_eq!(
            preferred_capture_source(&cameras),
            (cameras.rgb.clone(), false)
        );

        cameras.rgb = CameraSources::Ordered(vec![]);
        cameras.ir = "/dev/video2".into();
        assert!(resolve_configured_sources(&cameras).rgb.is_empty());
        assert_eq!(
            preferred_capture_source(&cameras),
            ("/dev/video2".into(), true)
        );
    }

    #[test]
    fn auto_parallel_capture_accounts_for_every_rgb_fallback() {
        let mut cameras = cameras_with("primary", "/dev/video2");
        cameras.rgb = CameraSources::Ordered(vec!["/dev/video0".into(), "/dev/video2".into()]);
        cameras.parallel_capture = "auto".into();
        assert!(resolve_configured_sources(&cameras).serial_capture);
        cameras.parallel_capture = "always".into();
        assert!(!resolve_configured_sources(&cameras).serial_capture);
    }

    fn cameras_with(rgb: &str, ir: &str) -> CameraConfig {
        CameraConfig {
            rgb: rgb.into(),
            ir: ir.to_string(),
            ir_frame_width: None,
            ir_frame_height: None,
            ir_source_marker: None,
            ir_source_driver: None,
            emitter_enabled: false,
            dark_luma_threshold: 30,
            parallel_capture: "never".to_string(),
            auto_rotate: true,
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
    fn configured_sources_resolve_an_ir_device_node() {
        let cameras = CameraConfig {
            rgb: "primary".into(),
            ir: "/dev/video2".to_string(),
            ir_frame_width: None,
            ir_frame_height: None,
            ir_source_marker: None,
            ir_source_driver: None,
            emitter_enabled: true,
            dark_luma_threshold: 30,
            parallel_capture: "never".to_string(),
            auto_rotate: true,
        };
        let sources = resolve_configured_sources(&cameras);
        assert_eq!(sources.rgb, "primary".into());
        assert_eq!(sources.ir, "/dev/video2");
        assert_eq!(sources.ir_node, "/dev/video2");
    }

    #[test]
    fn preferred_capture_source_falls_back_to_ir_when_rgb_is_unset() {
        let cameras = CameraConfig {
            rgb: "".into(),
            ir: "/dev/video2".to_string(),
            ir_frame_width: None,
            ir_frame_height: None,
            ir_source_marker: None,
            ir_source_driver: None,
            emitter_enabled: true,
            dark_luma_threshold: 30,
            parallel_capture: "never".to_string(),
            auto_rotate: true,
        };
        assert_eq!(
            preferred_capture_source(&cameras),
            ("/dev/video2".into(), true)
        );
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
            rgb: "primary".into(),
            ir: "pipewiresrc target-object=device-name".to_string(),
            ir_frame_width: None,
            ir_frame_height: None,
            ir_source_marker: None,
            ir_source_driver: None,
            emitter_enabled: true,
            dark_luma_threshold: 30,
            parallel_capture: "auto".to_string(),
            auto_rotate: true,
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
    fn privileged_capture_keeps_canonical_kernel_nodes() {
        assert_eq!(
            resolve_privileged_node("/dev/video0", true),
            Some("/dev/video0".to_string())
        );
        assert_eq!(
            resolve_privileged_node("  /dev/video2  ", false),
            Some("/dev/video2".to_string())
        );
    }

    #[test]
    fn privileged_capture_rejects_untrusted_or_malformed_sources() {
        for source in [
            "",
            "videotestsrc num-buffers=2",
            "v4l2src device=/dev/video0",
            "/dev/video",
            "/dev/video2 ! fakesink",
            "/dev/videoX",
            "usb:046d",
            "usb:zzzz:085e",
            "pipewiresrc fd=7",
            "pipewiresrcfoo",
            "pipewiresrc target-object=",
        ] {
            assert_eq!(
                resolve_privileged_node(source, true),
                None,
                "{source:?} must not resolve for privileged capture"
            );
        }
    }

    #[test]
    fn only_unplugged_v4l2_cameras_count_as_disconnected() {
        assert!(camera_disconnected("/dev/video9999", false));
        assert!(camera_disconnected(
            "pipewiresrc target-object=v4l2_input.pci-0000_ff_1f.7-usb-0_9.9.9_1.0.3",
            false
        ));
        for source in [
            "videotestsrc num-buffers=2",
            "pipewiresrc target-object=alsa_input.pci-0000_00_1f.3",
            "pipewiresrc target-object=51",
            "/dev/videofoo",
            "",
        ] {
            assert!(!camera_disconnected(source, false), "{source:?}");
        }
    }

    #[test]
    fn libcamera_fallback_only_covers_primary_color_capture() {
        assert!(libcamera_fallback_applies("primary", true, None));
        assert!(libcamera_fallback_applies("  primary ", true, None));
        assert!(!libcamera_fallback_applies("primary", false, None));
        for source in [
            "/dev/video0",
            "usb:046d:085e",
            "pipewiresrc",
            "pipewiresrc target-object=device-name",
            "videotestsrc",
            "libcamerasrc",
        ] {
            assert!(
                !libcamera_fallback_applies(source, true, None),
                "{source:?} must keep its own failure"
            );
        }
    }

    #[test]
    fn libcamera_negotiation_failures_point_at_the_software_isp() {
        let hint = libcamera_negotiation_hint(
            "Internal data stream error.: streaming stopped, reason not-negotiated (-4)",
        )
        .expect("not-negotiated gets a hint");
        assert!(hint.contains("Failed to fork"));
        assert!(
            libcamera_negotiation_hint("Failed to configure camera: Invalid argument").is_none()
        );
    }

    #[test]
    fn libcamera_fallback_skips_nodes_that_are_not_media_controller_only() {
        assert!(!libcamera_fallback_applies(
            "primary",
            true,
            Some("/dev/video-gaze-test-missing")
        ));
    }

    #[test]
    fn the_querycap_code_matches_the_kernels_ior_encoding() {
        let expected = (2 << 30)
            | ((std::mem::size_of::<V4l2Capability>() as libc::c_ulong) << 16)
            | ((b'V' as libc::c_ulong) << 8);
        assert_eq!(std::mem::size_of::<V4l2Capability>(), 104);
        assert_eq!(VIDIOC_QUERYCAP, expected);
    }

    #[test]
    fn device_caps_are_used_only_when_the_driver_reports_them() {
        let mc_only = V4L2_CAP_IO_MC | 0x1;
        assert_eq!(
            effective_device_caps(V4L2_CAP_DEVICE_CAPS | 0x1, mc_only),
            mc_only
        );
        assert_eq!(effective_device_caps(0x1, mc_only), 0x1);
    }

    #[test]
    fn libcamera_camera_names_survive_pipeline_parsing() {
        assert_eq!(
            libcamera_source_element("\\_SB_.LNK1"),
            "libcamerasrc camera-name=\"\\\\_SB_.LNK1\""
        );
        assert_eq!(
            libcamera_source_element("a \"b\""),
            "libcamerasrc camera-name=\"a \\\"b\\\"\""
        );
    }

    #[test]
    fn a_virtual_node_without_a_bridge_marker_stays_refused() {
        let missing = "/dev/video-gaze-test-missing";
        let err = check_trusted_node(missing, None).unwrap_err();
        assert!(
            err.to_string().contains("not a physical PCI/USB camera"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn a_bridge_marker_must_be_root_owned_owner_writable_and_name_a_video_node() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "gaze-bridge-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let marker = root.join("marker");
        let marker = marker.to_str().unwrap();
        // Content checks only run for a marker that passes the root-ownership gate, which a
        // test file only does when the suite itself runs as root.
        let as_root = unsafe { libc::geteuid() } == 0;

        std::fs::write(marker, "not-a-node\n").unwrap();
        std::fs::set_permissions(marker, std::fs::Permissions::from_mode(0o600)).unwrap();
        let err = bridge_marker_source(marker).unwrap_err();
        let expected = if as_root {
            "not a /dev/video* node"
        } else {
            "writable only"
        };
        assert!(
            err.to_string().contains(expected),
            "unexpected error: {err:#}"
        );

        if as_root {
            std::fs::write(marker, "/dev/video9999\n").unwrap();
            let err = bridge_marker_source(marker).unwrap_err();
            assert!(
                err.to_string().contains("not present"),
                "unexpected error: {err:#}"
            );
        }

        std::fs::set_permissions(marker, std::fs::Permissions::from_mode(0o666)).unwrap();
        let err = bridge_marker_source(marker).unwrap_err();
        assert!(
            err.to_string().contains("writable only"),
            "unexpected error: {err:#}"
        );

        assert!(bridge_marker_source(&root.join("missing").to_string_lossy()).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn acl_write_detection_covers_named_entries_only() {
        fn entry(tag: u16, perm: u16) -> [u8; 8] {
            let mut entry = [0u8; 8];
            entry[0..2].copy_from_slice(&tag.to_le_bytes());
            entry[2..4].copy_from_slice(&perm.to_le_bytes());
            entry[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
            entry
        }
        let blob = |entries: &[[u8; 8]]| {
            let mut blob = vec![2u8, 0, 0, 0];
            for entry in entries {
                blob.extend_from_slice(entry);
            }
            blob
        };
        // Owning user, owning group, mask, and other entries mirror the mode bits and
        // are not untrusted access.
        assert!(!acl_xattr_grants_named_write(&blob(&[
            entry(0x01, 0x6),
            entry(0x04, 0x6),
            entry(0x10, 0x6),
            entry(0x20, 0x4),
        ])));
        // A named user or group with write (for example the uaccess seat user) is.
        assert!(acl_xattr_grants_named_write(&blob(&[
            entry(0x01, 0x6),
            entry(0x02, 0x6),
            entry(0x10, 0x6),
        ])));
        assert!(acl_xattr_grants_named_write(&blob(&[
            entry(0x01, 0x6),
            entry(0x08, 0x2),
            entry(0x10, 0x6),
        ])));
        // Named entries are limited by the mask.
        assert!(!acl_xattr_grants_named_write(&blob(&[
            entry(0x01, 0x6),
            entry(0x02, 0x6),
            entry(0x10, 0x4),
        ])));
        // Without a mask entry, named permissions apply directly.
        assert!(acl_xattr_grants_named_write(&blob(&[
            entry(0x01, 0x6),
            entry(0x02, 0x2),
        ])));
        assert!(!acl_xattr_grants_named_write(&blob(&[
            entry(0x02, 0x5),
            entry(0x10, 0x6),
        ])));
        assert!(!acl_xattr_grants_named_write(&[2, 0, 0, 0]));
        assert!(!acl_xattr_grants_named_write(&[2, 0, 0]));
    }
}
