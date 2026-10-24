// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gstreamer::prelude::*;
use opencv::core::Mat;
use opencv::prelude::*;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tracing::{error, info, warn};

mod enumerate;
mod source;

pub use enumerate::*;
pub use source::*;

const REALTEK_IR_YUY2_WIDTH: u32 = 640;
const REALTEK_IR_YUY2_HEIGHT: u32 = 480;
const REQUIRED_CAMERA_ELEMENTS: [&str; 7] = [
    "appsink",
    "decodebin",
    "videoconvert",
    "videoscale",
    "jpegdec",
    "v4l2src",
    "pipewiresrc",
];

/// Lists the GStreamer elements required by Gaze's camera pipelines. Linker-based
/// scanners cannot detect these plugin dependencies, so packaging and `gaze doctor`
/// check for them at runtime.
pub fn missing_camera_elements() -> anyhow::Result<Vec<&'static str>> {
    gstreamer::init()?;
    Ok(REQUIRED_CAMERA_ELEMENTS
        .iter()
        .copied()
        .filter(|element| gstreamer::ElementFactory::find(element).is_none())
        .collect())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraKind {
    Rgb { source: String },
    Ir { source: String, node: String },
}

const PRIMARY_CAMERA_DISPLAY_NAME: &str = "Primary camera";
pub const IR_NONE_DISPLAY_NAME: &str = "None";
pub const RGB_NONE_DISPLAY_NAME: &str = "None (IR only)";
const DEVICE_SETTLE_TIMEOUT_MS: u64 = 100;
const INTERRUPTIBLE_POLL_TIMEOUT_MS: u64 = 100;
/// Gives a busy device time to reject the stream without making us wait indefinitely for a
/// live source that has not produced its first buffer. The frame loop handles later failures.
const PIPELINE_START_TIMEOUT_MS: u64 = 500;
/// PipeWire can hold the backing node for a moment after the GUI stops its preview.
const PRIVILEGED_BUSY_ATTEMPTS: u32 = 3;
const PRIVILEGED_BUSY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(250);
const PIPELINE_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

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

/// Identifies the uid whose PipeWire session capture should use, or `None` to resolve the socket
/// from the environment. Threads share only the uid; each pipeline opens its own socket.
static PIPEWIRE_UID: Mutex<Option<u32>> = Mutex::new(None);

thread_local! {
/// Returns the uid assigned to a capture thread, which takes precedence over the default. A thread
/// that has not opened its device yet must keep its original uid if a later claim rebinds the default.
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

fn retry_while_busy<T>(
    node: &str,
    delay: std::time::Duration,
    mut open: impl FnMut() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let mut attempt = 1;
    loop {
        match open() {
            Err(err)
                if attempt < PRIVILEGED_BUSY_ATTEMPTS && device_is_busy(&format!("{err:#}")) =>
            {
                warn!(
                    attempt,
                    ?delay,
                    "Privileged capture found {node} busy; retrying"
                );
                std::thread::sleep(delay);
                attempt += 1;
            }
            result => return result,
        }
    }
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

/// Describes which V4L2 source a failed PipeWire open may retry. A bare element, as used by
/// `primary`, can select any camera; a named target may retry only its own node, not another camera.
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

struct V4l2Retry {
    fallback: V4l2Fallback,
    camera_source: String,
    want_color: bool,
    frame_size: Option<(u32, u32)>,
}

pub struct Camera {
    pipeline: gstreamer::Pipeline,
    appsink: gstreamer_app::AppSink,
    /// Keeps the socket open while the pipeline can use it. GStreamer caches its `pw_core` by
    /// fd number, so recycling the number early could hand a later capture this one's core.
    _pipewire: Option<PipeWireSession>,
    /// Records why the stream ended and when the pipeline reported it. Without this detail, a
    /// stopped frame loop could look like an attempt that simply never detected a face.
    stream_error: Option<String>,
    fps: Mutex<Option<f64>>,
    v4l2_retry: Option<V4l2Retry>,
}

fn stop_pipeline(pipeline: &gstreamer::Pipeline) {
    if let Err(err) = pipeline.set_state(gstreamer::State::Null) {
        warn!("Failed to stop camera pipeline: {err}");
        return;
    }

    let (result, current, pending) = pipeline.state(Some(gstreamer::ClockTime::from_seconds(2)));
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

impl Drop for Camera {
    fn drop(&mut self) {
        let pipeline = self.pipeline.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("gaze-camera-stop".into())
            .spawn(move || {
                stop_pipeline(&pipeline);
                let _ = done_tx.send(());
            });
        if let Err(err) = spawned {
            warn!("Could not spawn camera stop thread ({err}); stopping inline");
            stop_pipeline(&self.pipeline);
            return;
        }

        // pipewiresrc can block set_state(Null) forever when stopped mid-renegotiation
        // (gst-plugin-pipewire 1.6.9), which would otherwise hang the capture thread silently.
        if done_rx.recv_timeout(PIPELINE_STOP_TIMEOUT).is_err() {
            error!(
                timeout_ms = PIPELINE_STOP_TIMEOUT.as_millis() as u64,
                "Camera pipeline is stuck stopping; abandoning it so capture can continue"
            );
            // The stuck pipeline still uses this socket, and pw_core is cached by fd number.
            std::mem::forget(self._pipewire.take());
        }
    }
}

pub fn frame_to_bytes(frame: &Mat) -> anyhow::Result<Vec<u8>> {
    let sz = frame.size()?;
    anyhow::ensure!(
        frame.typ() == opencv::core::CV_8UC3,
        "expected an 8-bit 3-channel Mat, got type {}",
        frame.typ()
    );
    anyhow::ensure!(frame.is_continuous(), "Mat rows are not tightly packed");

    let expected = (sz.width.max(0) as usize)
        .checked_mul(sz.height.max(0) as usize)
        .and_then(|pixels| pixels.checked_mul(3))
        .ok_or_else(|| anyhow::anyhow!("Mat dimensions overflow a byte count"))?;
    let bytes = frame.data_bytes()?;
    anyhow::ensure!(
        bytes.len() == expected,
        "Mat holds {} bytes, expected {expected}",
        bytes.len()
    );

    Ok(bytes.to_vec())
}

fn video_info_fps(video_info: &gstreamer_video::VideoInfo) -> Option<f64> {
    let fraction = video_info.fps();
    let (num, denom) = (fraction.numer(), fraction.denom());
    (num > 0 && denom > 0).then(|| num as f64 / denom as f64)
}

fn mirrored_bgr_frame(
    buffer: &gstreamer::BufferRef,
    info: &gstreamer_video::VideoInfo,
) -> anyhow::Result<Mat> {
    anyhow::ensure!(
        info.format() == gstreamer_video::VideoFormat::Bgr,
        "Expected BGR format, got {:?}",
        info.format()
    );
    let width = i32::try_from(info.width())?;
    let height = i32::try_from(info.height())?;
    anyhow::ensure!(width > 0 && height > 0, "Empty video frame");
    // VideoMeta can specify padding and an offset different from the negotiated caps.
    let (stride, offset) = if let Some(meta) = buffer.meta::<gstreamer_video::VideoMeta>() {
        anyhow::ensure!(
            meta.format() == info.format()
                && meta.width() == info.width()
                && meta.height() == info.height()
                && meta.n_planes() == 1,
            "Video metadata does not match BGR caps"
        );
        (meta.stride()[0], meta.offset()[0])
    } else {
        (info.stride()[0], info.offset()[0])
    };
    let stride = usize::try_from(stride)?;
    let row_bytes = (width as usize)
        .checked_mul(3)
        .ok_or_else(|| anyhow::anyhow!("Video row size overflow"))?;
    anyhow::ensure!(
        stride >= row_bytes,
        "Video stride is smaller than a BGR row"
    );
    let end = stride
        .checked_mul(height as usize)
        .and_then(|size| offset.checked_add(size))
        .ok_or_else(|| anyhow::anyhow!("Video frame size overflow"))?;
    let map = buffer
        .map_readable()
        .map_err(|_| anyhow::anyhow!("Buffer is not readable"))?;
    let pixels = map
        .get(offset..end)
        .ok_or_else(|| anyhow::anyhow!("Video buffer is smaller than its declared frame"))?;
    // Bounds, dimensions and stride are checked before OpenCV borrows this memory.
    // The map stays alive until flip has copied the frame into an owned Mat.
    let frame = unsafe {
        Mat::new_rows_cols_with_data_unsafe(
            height,
            width,
            opencv::core::CV_8UC3,
            pixels.as_ptr() as *mut std::ffi::c_void,
            stride,
        )?
    };
    let mut mirrored = Mat::default();
    opencv::core::flip(&frame, &mut mirrored, 1)?;
    Ok(mirrored)
}

impl Camera {
    pub fn open(camera_source: &str) -> anyhow::Result<Self> {
        Self::open_kind(camera_source, true, None)
    }

    pub fn open_ir(camera_source: &str, frame_size: Option<(u32, u32)>) -> anyhow::Result<Self> {
        Self::open_kind(camera_source, false, frame_size)
    }

    pub fn open_privileged(camera_source: &str) -> anyhow::Result<Self> {
        Self::open_privileged_kind(camera_source, true, None)
    }

    pub fn open_ir_privileged(
        camera_source: &str,
        frame_size: Option<(u32, u32)>,
    ) -> anyhow::Result<Self> {
        Self::open_privileged_kind(camera_source, false, frame_size)
    }

    fn open_with_frame_size_fallback(
        camera_source: &str,
        frame_size: Option<(u32, u32)>,
        mut open: impl FnMut(Option<(u32, u32)>) -> anyhow::Result<Self>,
    ) -> anyhow::Result<Self> {
        match (open(frame_size), frame_size) {
            (Err(err), Some((width, height))) if !device_is_busy(&format!("{err:#}")) => {
                warn!(
                    "IR frame size {width}x{height} failed for {camera_source} ({err:#}); \
                     falling back to auto-negotiation"
                );
                open(None)
            }
            (result, _) => result,
        }
    }

    fn open_privileged_kind(
        camera_source: &str,
        want_color: bool,
        frame_size: Option<(u32, u32)>,
    ) -> anyhow::Result<Self> {
        gstreamer::init()?;
        let Some(node) = resolve_privileged_node(camera_source, want_color) else {
            let err = if camera_disconnected(camera_source, want_color) {
                anyhow::anyhow!("camera {camera_source:?} is not connected")
            } else {
                anyhow::anyhow!(
                    "refusing privileged capture of {camera_source:?}: no backing /dev/video node \
                     (PipeWire sessions and custom pipelines are not trusted for authentication)"
                )
            };
            return Self::open_libcamera_fallback(camera_source, want_color, None, err);
        };
        let force_ir_yuy2 = node_requires_forced_ir_yuy2(&node, want_color);
        let src_element = format!("v4l2src device={node}");
        Self::open_with_frame_size_fallback(camera_source, frame_size, |size| {
            retry_while_busy(&node, PRIVILEGED_BUSY_RETRY_DELAY, || {
                Self::open_source_element(&src_element, camera_source, force_ir_yuy2, None, size)
            })
        })
        .or_else(|err| Self::open_libcamera_fallback(camera_source, want_color, Some(&node), err))
    }

    fn open_libcamera_fallback(
        camera_source: &str,
        want_color: bool,
        failed_node: Option<&str>,
        err: anyhow::Error,
    ) -> anyhow::Result<Self> {
        if !libcamera_fallback_applies(camera_source, want_color, failed_node) {
            return Err(err);
        }
        let Some(src_element) = libcamera_color_source() else {
            return Err(err);
        };
        warn!(
            "Privileged V4L2 capture of {camera_source:?} failed ({err:#}); trying {src_element}"
        );
        Self::open_source_element(&src_element, camera_source, false, None, None).map_err(
            |libcamera_err| {
                let libcamera_err = format!("{libcamera_err:#}");
                let hint = libcamera_negotiation_hint(&libcamera_err).unwrap_or_default();
                anyhow::anyhow!("{err:#}; libcamera fallback also failed: {libcamera_err}{hint}")
            },
        )
    }

    fn open_kind(
        camera_source: &str,
        want_color: bool,
        frame_size: Option<(u32, u32)>,
    ) -> anyhow::Result<Self> {
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

        let open = |size: Option<(u32, u32)>| -> anyhow::Result<Camera> {
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
            Self::open_source_element(&bound, camera_source, force_ir_yuy2, session, size)
        };

        let fallback = v4l2_fallback_for(&src_element);
        match Self::open_with_frame_size_fallback(camera_source, frame_size, open) {
            Ok(mut camera) => {
                if fallback != V4l2Fallback::None {
                    camera.v4l2_retry = Some(V4l2Retry {
                        fallback,
                        camera_source: camera_source.to_string(),
                        want_color,
                        frame_size,
                    });
                }
                Ok(camera)
            }
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
                let src_element = format!("v4l2src device={node}");
                Self::open_with_frame_size_fallback(camera_source, frame_size, |size| {
                    Self::open_source_element(
                        &src_element,
                        camera_source,
                        force_ir_yuy2,
                        None,
                        size,
                    )
                })
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
        frame_size: Option<(u32, u32)>,
    ) -> anyhow::Result<Self> {
        let pipeline_str = camera_pipeline(src_element, force_ir_yuy2, frame_size);
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
            v4l2_retry: None,
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

        if let Some(fps_val) = video_info_fps(&video_info)
            && let Ok(mut guard) = self.fps.lock()
            && guard.is_none()
        {
            *guard = Some(fps_val);
        }

        mirrored_bgr_frame(buffer, &video_info)
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

    pub fn fall_back_to_v4l2(&mut self) -> Option<String> {
        let retry = self.v4l2_retry.take()?;
        let node = match &retry.fallback {
            V4l2Fallback::AnyDevice => first_v4l2_node(retry.want_color),
            V4l2Fallback::SameNode(target) => node_from_pipewire_target(target, retry.want_color),
            V4l2Fallback::None => None,
        }?;

        if let Err(err) = self.pipeline.set_state(gstreamer::State::Null) {
            warn!("Failed to stop the PipeWire pipeline before the V4L2 retry: {err}");
            return None;
        }
        let _ = self
            .pipeline
            .state(Some(gstreamer::ClockTime::from_seconds(2)));
        self._pipewire = None;

        let force_ir_yuy2 = node_requires_forced_ir_yuy2(&node, retry.want_color);
        let src_element = format!("v4l2src device={node}");
        match Self::open_with_frame_size_fallback(&retry.camera_source, retry.frame_size, |size| {
            Self::open_source_element(
                &src_element,
                &retry.camera_source,
                force_ir_yuy2,
                None,
                size,
            )
        }) {
            Ok(camera) => {
                info!("Retrying the dark PipeWire stream on V4L2 camera node {node}");
                *self = camera;
                Some(node)
            }
            Err(err) => {
                warn!("V4L2 retry for {} failed: {err:#}", retry.camera_source);
                self.stream_error = Some(format!(
                    "The camera streamed only dark frames and its V4L2 node could not be opened: {err}"
                ));
                None
            }
        }
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
            && let Some(fps_val) = video_info_fps(&video_info)
        {
            if let Ok(mut guard) = self.fps.lock() {
                *guard = Some(fps_val);
            }
            return fps_val;
        }
        gaze_core::config::DEFAULT_CAMERA_FPS
    }

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

fn camera_pipeline(
    src_element: &str,
    force_ir_yuy2: bool,
    frame_size: Option<(u32, u32)>,
) -> String {
    if force_ir_yuy2 {
        // Dell/Realtek single-node RGB/IR modules silently stay in RGB mode unless the
        // stream is negotiated as uncompressed YUY2 at this exact resolution.
        let (width, height) = frame_size.unwrap_or((REALTEK_IR_YUY2_WIDTH, REALTEK_IR_YUY2_HEIGHT));
        format!(
            "{src_element} ! video/x-raw,format=YUY2,width={width},height={height},pixel-aspect-ratio=1/1 ! videoconvert ! videoscale ! appsink name=gaze_sink"
        )
    } else {
        let size = frame_size
            .map(|(width, height)| format!(",width={width},height={height}"))
            .unwrap_or_default();
        format!(
            "{src_element} ! video/x-raw{size},pixel-aspect-ratio=1/1; image/jpeg{size} ! decodebin ! videoconvert ! videoscale ! appsink name=gaze_sink"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_camera_buffer_is_rejected_before_opencv_borrows_it() {
        gstreamer::init().unwrap();
        let info = gstreamer_video::VideoInfo::builder(gstreamer_video::VideoFormat::Bgr, 2, 2)
            .build()
            .unwrap();
        let buffer = gstreamer::Buffer::from_mut_slice(vec![0u8; 1]);
        let err = mirrored_bgr_frame(&buffer, &info).unwrap_err();
        assert!(err.to_string().contains("smaller"), "{err}");
    }

    #[test]
    fn camera_metadata_cannot_supply_a_negative_short_or_overflowing_layout() {
        gstreamer::init().unwrap();
        let info = gstreamer_video::VideoInfo::builder(gstreamer_video::VideoFormat::Bgr, 2, 2)
            .build()
            .unwrap();
        for (offset, stride) in [(0, -8), (0, 3), (usize::MAX, 8), (16, 8)] {
            let mut buffer = gstreamer::Buffer::from_mut_slice(vec![0u8; 16]);
            gstreamer_video::VideoMeta::add_full(
                buffer.get_mut().unwrap(),
                gstreamer_video::VideoFrameFlags::empty(),
                gstreamer_video::VideoFormat::Bgr,
                2,
                2,
                &[offset],
                &[stride],
            )
            .unwrap();
            assert!(
                mirrored_bgr_frame(&buffer, &info).is_err(),
                "{offset}/{stride}"
            );
        }
    }

    #[test]
    fn camera_frames_honor_metadata_offset_and_stride_and_copy_the_pixels() {
        gstreamer::init().unwrap();
        let info = gstreamer_video::VideoInfo::builder(gstreamer_video::VideoFormat::Bgr, 2, 2)
            .build()
            .unwrap();
        let mut buffer = gstreamer::Buffer::from_mut_slice(vec![
            99, 99, 99, 99, 99, 99, 99, 99, 1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0,
        ]);
        gstreamer_video::VideoMeta::add_full(
            buffer.get_mut().unwrap(),
            gstreamer_video::VideoFrameFlags::empty(),
            gstreamer_video::VideoFormat::Bgr,
            2,
            2,
            &[8],
            &[8],
        )
        .unwrap();
        let frame = mirrored_bgr_frame(&buffer, &info).unwrap();
        drop(buffer);
        assert_eq!(
            frame_to_bytes(&frame).unwrap(),
            [4, 5, 6, 1, 2, 3, 10, 11, 12, 7, 8, 9]
        );
    }

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
        assert!(device_is_busy(
            "The camera is already in use by another program (primary): \
             Device '/dev/video0' is busy"
        ));
        assert!(!device_is_busy("No such file or directory"));
    }

    #[test]
    fn a_briefly_busy_node_is_opened_on_retry() {
        let mut calls = 0;
        let result = retry_while_busy("/dev/video0", std::time::Duration::ZERO, || {
            calls += 1;
            if calls < PRIVILEGED_BUSY_ATTEMPTS {
                anyhow::bail!("Device '/dev/video0' is busy")
            }
            Ok(calls)
        });
        assert_eq!(result.unwrap(), PRIVILEGED_BUSY_ATTEMPTS);
    }

    #[test]
    fn a_node_that_stays_busy_fails_after_the_last_attempt() {
        let mut calls = 0;
        let result: anyhow::Result<()> =
            retry_while_busy("/dev/video0", std::time::Duration::ZERO, || {
                calls += 1;
                anyhow::bail!("Device '/dev/video0' is busy")
            });
        assert!(device_is_busy(&format!("{:#}", result.unwrap_err())));
        assert_eq!(calls, PRIVILEGED_BUSY_ATTEMPTS);
    }

    #[test]
    fn other_open_errors_are_not_retried() {
        let mut calls = 0;
        let result: anyhow::Result<()> =
            retry_while_busy("/dev/video0", std::time::Duration::ZERO, || {
                calls += 1;
                anyhow::bail!("No such file or directory")
            });
        assert!(result.is_err());
        assert_eq!(calls, 1);
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

    #[test]
    fn realtek_ir_pipeline_forces_required_uncompressed_mode() {
        let pipeline = camera_pipeline("v4l2src device=/dev/video0", true, None);
        assert!(pipeline.contains("video/x-raw,format=YUY2,width=640,height=480"));
        assert!(!pipeline.contains("image/jpeg"));

        let rgb_pipeline = camera_pipeline("v4l2src device=/dev/video0", false, None);
        assert!(rgb_pipeline.contains("image/jpeg"));
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
        let mut camera = Camera::open("videotestsrc is-live=true ! identity error-after=5")
            .expect("identity pipeline");
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
    fn open_surfaces_malformed_sources() {
        assert!(Camera::open("/dev/video").is_err());
        assert!(Camera::open_ir("/dev/video2 ! fakesink", None).is_err());
        assert!(Camera::open("usb:046d").is_err());
    }

    #[test]
    fn privileged_open_refuses_custom_pipelines_without_touching_hardware() {
        let err = Camera::open_privileged("videotestsrc num-buffers=2")
            .err()
            .expect("privileged open of a custom pipeline must fail");
        assert!(
            err.to_string().contains("refusing privileged capture"),
            "unexpected error: {err:#}"
        );
        assert!(Camera::open_privileged("").is_err());
        assert!(Camera::open_ir_privileged("v4l2src device=/dev/video0", None).is_err());
    }

    #[test]
    fn ir_frame_size_constrains_raw_and_jpeg_caps() {
        let pipeline = camera_pipeline("v4l2src device=/dev/video2", false, Some((340, 340)));
        assert!(pipeline.contains("video/x-raw,width=340,height=340,pixel-aspect-ratio=1/1;"));
        assert!(pipeline.contains("image/jpeg,width=340,height=340 ! decodebin"));

        let forced = camera_pipeline("v4l2src device=/dev/video2", true, Some((340, 340)));
        assert!(forced.contains("video/x-raw,format=YUY2,width=340,height=340,"));
    }

    #[test]
    fn ir_frame_size_is_negotiated_when_the_source_supports_it() {
        let mut camera =
            Camera::open_ir("videotestsrc num-buffers=3", Some((480, 480))).expect("ir pipeline");
        let frame = camera.next().expect("videotestsrc frame");
        assert_eq!((frame.cols(), frame.rows()), (480, 480));
    }

    #[test]
    fn unsupported_ir_frame_size_falls_back_to_auto_negotiation() {
        let mut camera = Camera::open_ir(
            "videotestsrc num-buffers=3 ! capsfilter caps=video/x-raw,width=1280,height=720",
            Some((480, 480)),
        )
        .expect("fallback pipeline");
        let frame = camera.next().expect("videotestsrc frame");
        assert_eq!(frame.rows(), 480);
        let cols = frame.cols();
        assert!(
            (853..=854).contains(&cols),
            "expected the source's 16:9 aspect, got {cols}"
        );
    }
}
