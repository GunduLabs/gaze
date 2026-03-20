use gaze_core::camera::Camera;
use gaze_core::capture::frame_to_bytes;
use gaze_core::capture_session::CaptureHint;
use gtk4::gdk;
use gtk4::glib;
use gtk4::prelude::*;
use opencv::prelude::MatTraitConst;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use tracing::error;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

struct FrameData {
    rgb_bytes: Vec<u8>,
    width: i32,
    height: i32,
    mat: opencv::core::Mat,
}

pub struct CameraFeed {
    pub picture: gtk4::Picture,
    pub overlay_area: gtk4::DrawingArea,
    rx: Rc<RefCell<Option<mpsc::Receiver<FrameData>>>>,
    latest_frame: Rc<RefCell<Option<opencv::core::Mat>>>,
    status: Rc<RefCell<CaptureHint>>,
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
                    error!(%err, "Camera open failed");
                    return;
                }
            };

            while !stop_clone.load(Ordering::Relaxed) {
                let Ok(frame) = cam.capture_frame() else {
                    thread::sleep(std::time::Duration::from_millis(33));
                    continue;
                };

                let Ok(bytes) = frame_to_bytes(&frame) else {
                    continue;
                };

                let mut rgb = bytes;
                for chunk in rgb.chunks_exact_mut(3) {
                    chunk.swap(0, 2);
                }

                let Ok(size) = frame.size() else {
                    continue;
                };

                if tx
                    .send(FrameData {
                        rgb_bytes: rgb,
                        width: size.width,
                        height: size.height,
                        mat: frame,
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
            latest_frame: Rc::new(RefCell::new(None)),
            status: Rc::new(RefCell::new(CaptureHint::NoFace)),
            stop_flag,
        })
    }

    pub fn set_status(&self, new_status: CaptureHint) {
        *self.status.borrow_mut() = new_status;
        self.overlay_area.queue_draw();
    }

    pub fn take_frame(&self) -> Option<opencv::core::Mat> {
        self.latest_frame.borrow_mut().take()
    }

    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::Relaxed);
    }

    pub fn start(&self) {
        let rx = self
            .rx
            .borrow_mut()
            .take()
            .expect("CameraFeed already started");

        self.overlay_area.set_draw_func(glib::clone!(
            #[strong(rename_to = status)]
            self.status,
            move |_area, cr, width, height| {
                draw_face_guide(cr, width, height, &status.borrow());
            }
        ));

        glib::timeout_add_local(
            std::time::Duration::from_millis(33),
            glib::clone!(
                #[strong(rename_to = picture)]
                self.picture,
                #[strong(rename_to = latest_frame)]
                self.latest_frame,
                move || {
                    while let Ok(frame) = rx.try_recv() {
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

fn draw_face_guide(cr: &gtk4::cairo::Context, width: i32, height: i32, status: &CaptureHint) {
    let cx = width as f64 / 2.0;
    let cy = height as f64 / 2.0;

    let min_dim = width.min(height) as f64;
    let rx = min_dim * 0.28;
    let ry = min_dim * 0.38;

    let (red, green, blue, alpha) = match status {
        CaptureHint::NoFace => (0.6, 0.6, 0.6, 0.5),
        CaptureHint::NotCentered | CaptureHint::FaceClipped => (1.0, 0.8, 0.2, 0.7),
        CaptureHint::Ready => (0.2, 0.9, 0.4, 0.85),
    };

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

    let label = status.to_string();
    cr.set_font_size(min_dim * 0.035);
    let extents = cr.text_extents(&label).unwrap();
    cr.move_to(cx - extents.width() / 2.0, bottom + min_dim * 0.06);
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.9);
    let _ = cr.show_text(&label);
}

pub fn build_camera_widget(feed: &CameraFeed) -> gtk4::Overlay {
    let overlay = gtk4::Overlay::new();
    overlay.set_child(Some(&feed.picture));

    feed.overlay_area.set_hexpand(true);
    feed.overlay_area.set_vexpand(true);
    overlay.add_overlay(&feed.overlay_area);

    overlay
}
