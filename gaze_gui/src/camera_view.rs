use gaze_common::camera::Camera;
use gaze_common::capture::{CaptureResult, CaptureStatus, frame_to_bytes, try_capture};
use gaze_common::centering::FaceChecker;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct FrameData {
    rgb_bytes: Vec<u8>,
    width: i32,
    height: i32,
    status: CaptureStatusInfo,
    capture: Option<CaptureResult>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CaptureStatusInfo {
    NoFace,
    NotCentered,
    Centered,
}

pub struct CameraFeed {
    pub picture: gtk4::Picture,
    pub overlay_area: gtk4::DrawingArea,
    rx: Rc<RefCell<Option<mpsc::Receiver<FrameData>>>>,
    status: Rc<RefCell<CaptureStatusInfo>>,
    last_capture: Rc<RefCell<Option<CaptureResult>>>,
    stop_flag: Arc<AtomicBool>,
}

impl CameraFeed {
    pub fn new(device: &str) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::channel::<FrameData>();
        let device = device.to_string();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_flag.clone();

        thread::spawn(move || {
            let mut cam = match Camera::open(&device) {
                Ok(c) => c,
                Err(err) => {
                    eprintln!("Camera open failed: {}", err);
                    return;
                }
            };
            let mut checker = match FaceChecker::new() {
                Ok(c) => c,
                Err(err) => {
                    eprintln!("FaceChecker init failed: {}", err);
                    return;
                }
            };

            while !stop_clone.load(Ordering::Relaxed) {
                let capture_status = match try_capture(&mut cam, &mut checker) {
                    Ok(s) => s,
                    Err(_) => {
                        thread::sleep(std::time::Duration::from_millis(33));
                        continue;
                    }
                };

                let frame = match cam.capture_frame() {
                    Ok(f) => f,
                    Err(_) => {
                        thread::sleep(std::time::Duration::from_millis(33));
                        continue;
                    }
                };
                let fb = match frame_to_bytes(&frame) {
                    Ok(r) => r,
                    Err(_) => continue,
                };

                let mut rgb = vec![0u8; fb.bytes.len()];
                for idx in (0..fb.bytes.len()).step_by(3) {
                    if idx + 2 < fb.bytes.len() {
                        rgb[idx] = fb.bytes[idx + 2];
                        rgb[idx + 1] = fb.bytes[idx + 1];
                        rgb[idx + 2] = fb.bytes[idx];
                    }
                }

                let (status, capture) = match capture_status {
                    CaptureStatus::Ready(result) => (CaptureStatusInfo::Centered, Some(result)),
                    CaptureStatus::NotCentered => (CaptureStatusInfo::NotCentered, None),
                    CaptureStatus::NoFace => (CaptureStatusInfo::NoFace, None),
                };

                if tx
                    .send(FrameData {
                        rgb_bytes: rgb,
                        width: fb.width as i32,
                        height: fb.height as i32,
                        status,
                        capture,
                    })
                    .is_err()
                {
                    break;
                }

                thread::sleep(std::time::Duration::from_millis(33));
            }
            // camera dropped here, resources freed
        });

        let picture = gtk4::Picture::new();
        picture.set_content_fit(gtk4::ContentFit::Contain);

        let overlay_area = gtk4::DrawingArea::new();

        Ok(Self {
            picture,
            overlay_area,
            rx: Rc::new(RefCell::new(Some(rx))),
            status: Rc::new(RefCell::new(CaptureStatusInfo::NoFace)),
            last_capture: Rc::new(RefCell::new(None)),
            stop_flag,
        })
    }

    pub fn status(&self) -> CaptureStatusInfo {
        self.status.borrow().clone()
    }

    pub fn take_capture(&self) -> Option<CaptureResult> {
        self.last_capture.borrow_mut().take()
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }

    pub fn start(&self) {
        let picture = self.picture.clone();
        let overlay = self.overlay_area.clone();
        let status = self.status.clone();
        let last_capture = self.last_capture.clone();
        let rx = self
            .rx
            .borrow_mut()
            .take()
            .expect("CameraFeed already started");

        let status_for_draw = status.clone();
        overlay.set_draw_func(move |_area, cr, width, height| {
            draw_face_guide(cr, width, height, &status_for_draw.borrow());
        });

        glib::timeout_add_local(std::time::Duration::from_millis(33), move || {
            while let Ok(frame) = rx.try_recv() {
                *status.borrow_mut() = frame.status.clone();

                if let Some(cap) = frame.capture {
                    *last_capture.borrow_mut() = Some(cap);
                }

                let bytes = glib::Bytes::from(&frame.rgb_bytes);
                let texture = gdk::MemoryTexture::new(
                    frame.width,
                    frame.height,
                    gdk::MemoryFormat::R8g8b8,
                    &bytes,
                    (frame.width * 3) as usize,
                );
                picture.set_paintable(Some(&texture));
                overlay.queue_draw();
            }
            glib::ControlFlow::Continue
        });
    }
}

fn draw_face_guide(cr: &gtk4::cairo::Context, width: i32, height: i32, status: &CaptureStatusInfo) {
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    // Scale relative to whichever dimension is smaller
    let min_dim = width.min(height) as f64;
    let rx = min_dim * 0.28;
    let ry = min_dim * 0.38;

    let (red, green, blue, alpha) = match status {
        CaptureStatusInfo::NoFace => (0.6, 0.6, 0.6, 0.5),
        CaptureStatusInfo::NotCentered => (1.0, 0.8, 0.2, 0.7),
        CaptureStatusInfo::Centered => (0.2, 0.9, 0.4, 0.85),
    };

    // Oval
    cr.save().unwrap();
    cr.translate(cx, cy);
    cr.scale(rx, ry);
    cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * std::f64::consts::PI);
    cr.restore().unwrap();

    cr.set_source_rgba(red, green, blue, alpha * 0.08);
    let _ = cr.fill_preserve();

    cr.set_source_rgba(red, green, blue, alpha);
    cr.set_line_width(2.5);
    let _ = cr.stroke();

    let bracket_len = min_dim * 0.04;
    let left = cx - rx;
    let right = cx + rx;
    let top = cy - ry;
    let bottom = cy + ry;

    cr.set_source_rgba(red, green, blue, alpha);
    cr.set_line_width(2.5);

    for (bx, by, dx, dy) in [
        (left, top, 1.0, 1.0),
        (right, top, -1.0, 1.0),
        (left, bottom, 1.0, -1.0),
        (right, bottom, -1.0, -1.0),
    ] {
        cr.move_to(bx, by + dy * bracket_len);
        cr.line_to(bx, by);
        cr.line_to(bx + dx * bracket_len, by);
        let _ = cr.stroke();
    }

    // Status text below the oval
    let label = match status {
        CaptureStatusInfo::NoFace => "No face detected",
        CaptureStatusInfo::NotCentered => "Center your face",
        CaptureStatusInfo::Centered => "Hold still...",
    };
    cr.set_font_size(min_dim * 0.035);
    let extents = cr.text_extents(label).unwrap();
    cr.move_to(cx - extents.width() / 2.0, bottom + min_dim * 0.06);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
    let _ = cr.show_text(label);
}

pub fn build_camera_widget(feed: &CameraFeed) -> gtk4::Overlay {
    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&feed.picture));

    feed.overlay_area.set_hexpand(true);
    feed.overlay_area.set_vexpand(true);
    overlay.add_overlay(&feed.overlay_area);

    overlay
}
