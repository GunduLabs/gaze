use gaze_core::camera::{Camera, frame_to_bytes};
use gaze_core::dbus::CaptureStatus;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use opencv::prelude::MatTraitConst;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, TrySendError};
use std::thread;
use tracing::error;

struct FrameData {
    rgb_bytes: Vec<u8>,
    width: i32,
    height: i32,
    mat: opencv::core::Mat,
}

pub struct CameraFeed {
    pub picture: gtk4::Picture,
    pub overlay: gtk4::Overlay,
    guide: gtk4::DrawingArea,
    rx: Rc<RefCell<Option<mpsc::Receiver<FrameData>>>>,
    latest_frame: Rc<RefCell<Option<opencv::core::Mat>>>,
    thread_handle: RefCell<Option<thread::JoinHandle<()>>>,
    stop_flag: Arc<AtomicBool>,
    face_status: Rc<RefCell<CaptureStatus>>,
    is_active: Rc<RefCell<bool>>,
}

impl CameraFeed {
    pub fn new(device: &str) -> anyhow::Result<Self> {
        let (tx, rx) = mpsc::sync_channel::<FrameData>(1);
        let device = device.to_string();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_clone = stop_flag.clone();

        let thread_handle = thread::spawn(move || {
            let mut cam = match Camera::open(&device) {
                Ok(c) => c,
                Err(err) => {
                    error!(%err, "Camera open failed");
                    return;
                }
            };

            for frame in &mut cam {
                if stop_clone.load(Ordering::Relaxed) {
                    break;
                }

                let Ok(bytes) = frame_to_bytes(&frame) else {
                    continue;
                };

                // OpenCV gives us BGR; GTK's R8g8b8 texture format expects RGB, so swap each pixel.
                let mut rgb = bytes;
                for chunk in rgb.chunks_exact_mut(3) {
                    chunk.swap(0, 2);
                }

                let Ok(size) = frame.size() else {
                    continue;
                };

                let frame_data = FrameData {
                    rgb_bytes: rgb,
                    width: size.width,
                    height: size.height,
                    mat: frame,
                };
                match tx.try_send(frame_data) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Disconnected(_)) => break,
                }
            }
        });

        Ok(Self::assemble(Some(rx), Some(thread_handle), stop_flag))
    }

    /// A feed that renders only the face-position guide overlay, driven by the
    /// daemon's capture status. It never opens the camera, so it is safe for IR
    /// devices: the daemon is the sole owner of the raw V4L2 node, and a second
    /// local stream would fail with EBUSY while the emitter is lit only during
    /// the daemon's own capture window.
    pub fn new_guidance_only() -> Self {
        Self::assemble(None, None, Arc::new(AtomicBool::new(false)))
    }

    fn assemble(
        rx: Option<mpsc::Receiver<FrameData>>,
        thread_handle: Option<thread::JoinHandle<()>>,
        stop_flag: Arc<AtomicBool>,
    ) -> Self {
        let picture = gtk4::Picture::new();
        picture.set_content_fit(gtk4::ContentFit::Contain);

        let overlay = gtk4::Overlay::new();
        overlay.set_child(Some(&picture));

        let face_status = Rc::new(RefCell::new(CaptureStatus::NoFace));
        let is_active = Rc::new(RefCell::new(false));
        let draw_status = face_status.clone();
        let draw_active = is_active.clone();
        let guide = gtk4::DrawingArea::new();
        guide.set_draw_func(move |_area, cr, width, height| {
            let status = *draw_status.borrow();
            let active = *draw_active.borrow();

            let cx = width as f64 / 2.0;
            let cy = height as f64 / 2.0;
            let min_dim = width.min(height) as f64;
            let rx = min_dim * 0.28;
            let ry = min_dim * 0.38;

            let (red, green, blue, alpha) = if active {
                match status {
                    CaptureStatus::NoFace => (0.6, 0.6, 0.6, 0.5),
                    CaptureStatus::TooDark
                    | CaptureStatus::NotCentered
                    | CaptureStatus::Clipped
                    | CaptureStatus::TooFar
                    | CaptureStatus::TooClose => (1.0, 0.8, 0.2, 0.7),
                    CaptureStatus::Ready | CaptureStatus::Usable => (0.2, 0.9, 0.4, 0.85),
                }
            } else {
                (0.6, 0.6, 0.6, 0.4)
            };

            let _ = cr.save();
            cr.translate(cx, cy);
            cr.scale(rx, ry);
            cr.arc(0.0, 0.0, 1.0, 0.0, 2.0 * std::f64::consts::PI);
            let _ = cr.restore();

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

            if active {
                let label = match status {
                    CaptureStatus::NoFace => "No Face",
                    CaptureStatus::TooDark => "Need More Light",
                    CaptureStatus::NotCentered => "Not Centered",
                    CaptureStatus::Clipped => "Face Clipped",
                    CaptureStatus::TooFar => "Come Closer",
                    CaptureStatus::TooClose => "Back Up",
                    CaptureStatus::Ready | CaptureStatus::Usable => "Ready",
                };
                cr.set_font_size(min_dim * 0.035);
                if let Ok(extents) = cr.text_extents(label) {
                    cr.move_to(cx - extents.width() / 2.0, bottom + min_dim * 0.06);
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
                    let _ = cr.show_text(label);
                }
            }
        });
        overlay.add_overlay(&guide);

        Self {
            picture,
            overlay,
            guide,
            rx: Rc::new(RefCell::new(rx)),
            latest_frame: Rc::new(RefCell::new(None)),
            thread_handle: RefCell::new(thread_handle),
            stop_flag,
            face_status,
            is_active,
        }
    }

    pub fn set_face_status(&self, status: CaptureStatus) {
        *self.face_status.borrow_mut() = status;
        self.guide.queue_draw();
    }

    pub fn set_active(&self, active: bool) {
        *self.is_active.borrow_mut() = active;
        self.guide.queue_draw();
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.thread_handle.borrow_mut().take() {
            thread::spawn(move || {
                let _ = handle.join();
            });
        }
    }

    pub fn start(&self) {
        let Some(rx) = self.rx.borrow_mut().take() else {
            // Guidance-only feed (e.g. IR): no capture thread to pump.
            return;
        };

        glib::timeout_add_local(
            std::time::Duration::from_millis(33),
            glib::clone!(
                #[strong(rename_to = picture)]
                self.picture,
                #[strong(rename_to = latest_frame)]
                self.latest_frame,
                move || {
                    let mut newest = None;
                    while let Ok(frame) = rx.try_recv() {
                        newest = Some(frame);
                    }
                    if let Some(frame) = newest {
                        let bytes = glib::Bytes::from(&frame.rgb_bytes);
                        let texture = gdk::MemoryTexture::new(
                            frame.width,
                            frame.height,
                            gdk::MemoryFormat::R8g8b8,
                            &bytes,
                            (frame.width * 3) as usize,
                        );
                        picture.set_paintable(Some(&texture));
                        *latest_frame.borrow_mut() = Some(frame.mat);
                    }
                    glib::ControlFlow::Continue
                }
            ),
        );
    }
}

pub fn build_camera_widget(feed: &CameraFeed) -> gtk4::Overlay {
    let overlay = feed.overlay.clone();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay
}
