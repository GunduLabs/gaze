use crate::camera_view::{CameraFeed, CaptureStatusInfo, build_camera_widget};
use gaze_core::config::Config;
use gaze_core::dbus::AuthProxy;
use gaze_core::capture_session::{CaptureMode, CaptureState, CaptureSession};
use gaze_core::face::FaceChecker;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use tracing::error;

pub fn show_capture_dialog(
    parent: &impl IsA<gtk4::Widget>,
    username: &str,
    face_name: Option<&str>,
    proxy: &Rc<AuthProxy<'static>>,
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
        "New Face Capture Session"
    }));
    dialog.set_default_size(500, if is_refine { 550 } else { 630 });
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
    cam_frame.set_vexpand(true);
    body.append(&cam_frame);

    let prompt_label = gtk4::Label::new(None);
    prompt_label.add_css_class("title-4");
    body.append(&prompt_label);

    let progress_label = gtk4::Label::new(Some("Waiting to start"));
    progress_label.add_css_class("dim-label");
    progress_label.set_margin_bottom(2);
    body.append(&progress_label);

    let progress = gtk4::ProgressBar::new();
    body.append(&progress);

    let status_label = gtk4::Label::new(None);
    status_label.add_css_class("dim-label");
    body.append(&status_label);

    let start_btn = gtk4::Button::with_label(if is_refine {
        "Start Refining"
    } else {
        "Start Capture"
    });
    start_btn.add_css_class("suggested-action");
    start_btn.add_css_class("pill");
    start_btn.set_halign(gtk4::Align::Center);
    start_btn.set_sensitive(false);
    body.append(&start_btn);

    let stop_btn = gtk4::Button::with_label("Cancel");
    stop_btn.add_css_class("destructive-action");
    stop_btn.add_css_class("pill");
    stop_btn.set_halign(gtk4::Align::Center);
    stop_btn.set_visible(false);
    body.append(&stop_btn);

    content.append(&body);
    dialog.set_content(Some(&content));

    let current_step = Rc::new(RefCell::new(0usize));
    let username = username.to_string();

    let Ok(checker) = FaceChecker::new() else {
        error!("FaceChecker init failed");
        return;
    };

    let mode = if is_refine {
        CaptureMode::Refine
    } else {
        CaptureMode::Guided
    };
    let session = Rc::new(RefCell::new(CaptureSession::new(checker).with_mode(mode)));

    start_btn.connect_clicked(glib::clone!(
        #[strong]
        session,
        #[weak]
        stop_btn,
        #[weak]
        prompt_label,
        move |btn| {
            session.borrow_mut().start();
            btn.set_visible(false);
            stop_btn.set_visible(true);
            prompt_label.set_text("Capture Session in progress...");
        }
    ));

    stop_btn.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            dialog.close();
        }
    ));

    glib::MainContext::default().spawn_local(glib::clone!(
        #[weak]
        feed,
        #[weak]
        prompt_label,
        #[weak]
        progress,
        #[weak]
        progress_label,
        #[weak]
        status_label,
        #[weak]
        dialog,
        #[weak]
        start_btn,
        #[strong]
        on_done,
        #[strong]
        current_step,
        #[strong]
        session,
        #[strong]
        username,
        #[strong]
        resolved_face,
        #[strong]
        proxy,
        async move {
            loop {
                glib::timeout_future(std::time::Duration::from_millis(30)).await;

                if session.borrow().is_complete() {
                    let face_name = resolved_face.borrow().clone();
                    let captures = session.borrow_mut().take_captures();

                    for capture in captures {
                        if let Err(e) = proxy
                            .add_face(
                                &username,
                                &face_name,
                                &capture.bytes,
                                capture.width,
                                capture.height,
                            )
                            .await
                        {
                            error!(%e, "Failed to upload capture");
                        }
                    }

                    prompt_label.set_text("✓ Capture Session Complete!");
                    on_done();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(1000),
                        glib::clone!(
                            #[weak]
                            dialog,
                            move || {
                                dialog.close();
                            }
                        ),
                    );
                    break;
                }

                let mut state = None;
                if let Some(mat) = feed.take_frame() {
                    let mut capture_session = session.borrow_mut();
                    let res = capture_session.process_frame(&mat);
                    drop(capture_session);

                    match res {
                        Ok(s) => state = Some(s),
                        Err(e) => {
                            error!(%e, "Error processing frame");
                        }
                    }
                }

                if let Some(state) = state {
                    match state {
                        CaptureState::Prompting {
                            prompt,
                            step,
                            total_steps,
                            ..
                        }
                        | CaptureState::Countdown {
                            prompt,
                            step,
                            total_steps,
                            ..
                        } => {
                            prompt_label.set_text(&prompt.to_string());

                            if session.borrow().is_active() {
                                progress.set_fraction(step as f64 / total_steps as f64);
                                progress_label.set_text(&format!("{}/{}", step, total_steps));
                            } else {
                                progress.set_fraction(0.0);
                                progress_label.set_text("Waiting to start");
                            }

                            match state {
                                CaptureState::Prompting { hint, .. } => {
                                    let is_active = session.borrow().is_active();
                                    
                                    status_label.set_visible(false);

                                    let (status_info, is_centered) = match hint {
                                        gaze_core::capture_session::CaptureHint::FaceClipped => (CaptureStatusInfo::Clipped, false),
                                        gaze_core::capture_session::CaptureHint::NoFace => (CaptureStatusInfo::NoFace, false),
                                        gaze_core::capture_session::CaptureHint::NotCentered => (CaptureStatusInfo::NotCentered, false),
                                        gaze_core::capture_session::CaptureHint::CenteredReady => (CaptureStatusInfo::Centered, true),
                                    };

                                    feed.set_status(status_info);

                                    if !is_active {
                                        start_btn.set_sensitive(is_centered);
                                    }
                                }
                                CaptureState::Countdown {
                                    seconds_remaining, ..
                                } => {
                                    status_label.set_text(&format!(
                                        "✓ Centered! Hold still for {:.1}s...",
                                        seconds_remaining
                                    ));
                                    status_label.set_visible(true);
                                    feed.set_status(CaptureStatusInfo::Centered);
                                }
                                _ => unreachable!(),
                            }
                        }
                        CaptureState::Captured { .. } => {
                            feed.set_status(CaptureStatusInfo::Centered);
                            *current_step.borrow_mut() += 1;
                            status_label.set_text("✓ Captured!");
                            on_done();
                        }
                        CaptureState::Complete => {}
                    }
                }
            }
        }
    ));

    dialog.connect_close_request(glib::clone!(
        #[strong]
        feed,
        #[strong]
        on_done,
        #[strong]
        session,
        move |dialog| {
            if session.borrow().is_active() && !session.borrow().is_complete() {
                let heading_text = if is_refine {
                    "Cancel Refining?"
                } else {
                    "Cancel Capture Session?"
                };
                let confirm = libadwaita::MessageDialog::builder()
                    .heading(heading_text)
                    .body("This will discard any partial captures.")
                    .transient_for(dialog)
                    .build();

                confirm.add_response("resume", "Resume");
                confirm.add_response("discard", "Discard");
                confirm.set_response_appearance(
                    "discard",
                    libadwaita::ResponseAppearance::Destructive,
                );

                let dialog_clone = dialog.clone();
                confirm.connect_response(
                    None,
                    glib::clone!(
                        #[strong]
                        feed,
                        #[strong]
                        on_done,
                        #[strong]
                        session,
                        move |_, response| {
                            if response == "discard" {
                                feed.stop();
                                session.borrow_mut().stop();
                                on_done();
                                dialog_clone.close();
                            }
                        }
                    ),
                );
                confirm.present();
                glib::Propagation::Stop
            } else {
                feed.stop();
                on_done();
                glib::Propagation::Proceed
            }
        }
    ));

    dialog.present();
}
