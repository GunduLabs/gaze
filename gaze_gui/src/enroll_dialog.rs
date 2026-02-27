use crate::camera_view::{CameraFeed, CaptureStatusInfo, build_camera_widget};
use gaze_core::config::Config;
use gaze_core::dbus::AuthProxy;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use tracing::error;
use zbus::Connection;

const ENROLL_PROMPTS: &[&str] = &[
    "Look straight at the camera",
    "Turn your head slightly LEFT",
    "Turn your head slightly RIGHT",
    "Tilt your head slightly UP",
];

pub fn show_enroll_dialog(
    parent: &impl IsA<gtk4::Widget>,
    username: &str,
    face_name: Option<&str>,
    on_done: impl Fn() + 'static,
) {
    let config = Config::load().unwrap_or_default();
    let feed = match CameraFeed::new(&config.cameras.rgb) {
        Ok(f) => {
            f.start();
            f
        }
        Err(err) => {
            error!(%err, "Camera init failed");
            return;
        }
    };
    let feed = Rc::new(feed);
    let on_done = Rc::new(on_done);

    let is_refine = face_name.is_some();
    let dialog = libadwaita::Window::new();
    dialog.set_title(Some(if is_refine {
        "Refining Face"
    } else {
        "New Face Enrollment"
    }));
    dialog.set_default_size(500, 650);
    dialog.set_modal(true);
    dialog.set_transient_for(
        parent
            .root()
            .and_then(|r| r.downcast::<gtk4::Window>().ok())
            .as_ref(),
    );

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 0);
    let header = libadwaita::HeaderBar::new();
    content.append(&header);

    let body = gtk4::Box::new(gtk4::Orientation::Vertical, 12);
    body.set_margin_start(16);
    body.set_margin_end(16);
    body.set_margin_top(16);
    body.set_margin_bottom(16);

    let resolved_face = Rc::new(RefCell::new(face_name.unwrap_or("default").to_string()));
    if !is_refine {
        let entry = libadwaita::EntryRow::new();
        entry.set_title("Face Name");
        entry.set_text("default");
        let group = libadwaita::PreferencesGroup::new();
        group.add(&entry);
        body.append(&group);
        let rf = resolved_face.clone();
        entry.connect_changed(move |e| {
            *rf.borrow_mut() = e.text().to_string();
        });
    }

    let cam_widget = build_camera_widget(&feed);
    cam_widget.set_height_request(320);
    let cam_frame = gtk4::Frame::new(None);
    cam_frame.set_child(Some(&cam_widget));
    body.append(&cam_frame);

    let prompt_label = gtk4::Label::new(None);
    prompt_label.add_css_class("title-4");
    body.append(&prompt_label);

    let progress = gtk4::ProgressBar::new();
    progress.set_show_text(true);
    body.append(&progress);

    let status_label = gtk4::Label::new(None);
    status_label.add_css_class("dim-label");
    body.append(&status_label);

    let start_btn = gtk4::Button::with_label(if is_refine {
        "Start Refining"
    } else {
        "Start Enrollment"
    });
    start_btn.add_css_class("suggested-action");
    start_btn.add_css_class("pill");
    start_btn.set_halign(gtk4::Align::Center);
    body.append(&start_btn);

    let stop_btn = gtk4::Button::with_label("Stop Refining");
    stop_btn.add_css_class("destructive-action");
    stop_btn.add_css_class("pill");
    stop_btn.set_halign(gtk4::Align::Center);
    stop_btn.set_visible(false);
    body.append(&stop_btn);

    content.append(&body);
    dialog.set_content(Some(&content));

    let active = Rc::new(RefCell::new(false));
    let step = Rc::new(RefCell::new(0usize));
    let countdown = Rc::new(RefCell::new(0i32));
    let username = username.to_string();

    {
        let active = active.clone();
        let prompt_label = prompt_label.clone();
        let stop_btn_clone = stop_btn.clone();

        start_btn.connect_clicked(move |btn| {
            *active.borrow_mut() = true;
            btn.set_sensitive(false);
            if is_refine {
                btn.set_visible(false);
                stop_btn_clone.set_visible(true);
                prompt_label.set_text("Refining in progress...");
            }
        });
    }

    {
        let active = active.clone();
        let dialog_weak = glib::SendWeakRef::from(dialog.downgrade());
        stop_btn.connect_clicked(move |_| {
            *active.borrow_mut() = false;
            if let Some(dlg) = dialog_weak.upgrade() {
                dlg.close();
            }
        });
    }

    {
        let feed = feed.clone();
        let active = active.clone();
        let step = step.clone();
        let countdown = countdown.clone();
        let prompt_label = prompt_label.clone();
        let progress = progress.clone();
        let status_label = status_label.clone();
        let dialog_weak = glib::SendWeakRef::from(dialog.downgrade());
        let on_done = on_done.clone();

        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            if !*active.borrow() {
                return glib::ControlFlow::Continue;
            }

            if feed.status() != CaptureStatusInfo::Centered {
                status_label.set_text(match feed.status() {
                    CaptureStatusInfo::NoFace => "⏳ No face detected...",
                    _ => "⏳ Center your face...",
                });
                *countdown.borrow_mut() = 30;
                return glib::ControlFlow::Continue;
            }

            let mut cd = countdown.borrow_mut();
            if *cd > 0 {
                status_label.set_text(&format!(
                    "✓ Centered! Hold still for {:.1}s...",
                    *cd as f32 / 10.0
                ));
                *cd -= 1;
                return glib::ControlFlow::Continue;
            }
            *cd = 30;
            if let Some(cap) = feed.take_capture() {
                let username = username.clone();
                let face_name = resolved_face.borrow().clone();
                let step = step.clone();
                let active = active.clone();
                let prompt_label = prompt_label.clone();
                let progress = progress.clone();
                let status_label = status_label.clone();
                let dialog_weak = dialog_weak.clone();
                let on_done = on_done.clone();

                glib::MainContext::default().spawn_local(async move {
                    let Ok(conn) = Connection::system().await else {
                        return;
                    };
                    let Ok(proxy) = AuthProxy::new(&conn).await else {
                        return;
                    };

                    if proxy
                        .add_face(&username, &face_name, &cap.bytes, cap.width, cap.height)
                        .await
                        .is_ok()
                    {
                        let mut s = step.borrow_mut();
                        *s += 1;
                        let count = *s;

                        if is_refine {
                            progress.set_text(Some(&format!("{} captures", count)));
                            progress.set_fraction((count % 10) as f64 / 10.0);
                            on_done();
                        } else {
                            progress.set_fraction(count as f64 / ENROLL_PROMPTS.len() as f64);
                            progress.set_text(Some(&format!("{}/{}", count, ENROLL_PROMPTS.len())));
                            if count >= ENROLL_PROMPTS.len() {
                                *active.borrow_mut() = false;
                                prompt_label.set_text("✓ Enrollment Complete!");
                                on_done();
                                glib::timeout_add_local_once(
                                    std::time::Duration::from_millis(1000),
                                    move || {
                                        if let Some(d) = dialog_weak.upgrade() {
                                            d.close();
                                        }
                                    },
                                );
                                return;
                            }
                            prompt_label.set_text(ENROLL_PROMPTS[count]);
                        }
                        status_label.set_text("✓ Captured!");
                    }
                });
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let feed = feed.clone();
        let on_done = on_done.clone();
        dialog.connect_close_request(move |_| {
            feed.stop();
            on_done();
            glib::Propagation::Proceed
        });
    }

    dialog.present();
}
