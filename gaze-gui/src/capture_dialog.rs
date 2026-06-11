use crate::camera_view::{CameraFeed, build_camera_widget};
use futures::StreamExt;
use gaze_core::dbus::{EnrollPrompt, GazeProxy};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use tracing::error;

pub fn show_capture_dialog(
    parent: &impl IsA<gtk4::Widget>,
    username: &str,
    face_name: Option<&str>,
    proxy: &Rc<GazeProxy<'static>>,
    camera_device: &str,
    on_done: impl Fn() + 'static,
) {
    // IR cameras are driven straight off the raw V4L2 node by the daemon, which
    // is the device's sole owner. Opening a second local preview stream here
    // would fail with EBUSY (and the emitter is only lit during the daemon's own
    // capture), so for IR we show a guidance-only overlay fed by daemon status.
    let feed = if camera_device.starts_with("v4l2src") {
        CameraFeed::new_guidance_only()
    } else {
        match CameraFeed::new(camera_device) {
            Ok(f) => f,
            Err(err) => {
                error!(%err, "Camera init failed");
                return;
            }
        }
    };
    feed.start();
    let feed = Rc::new(feed);
    let on_done = Rc::new(on_done);

    let is_refine = face_name.is_some();

    let dialog = libadwaita::Window::new();
    dialog.set_title(Some(if is_refine {
        "Updating Face Template"
    } else {
        "New Face Template"
    }));
    dialog.set_default_size(500, if is_refine { 450 } else { 530 });
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

    let camera_mode = gtk4::Label::new(Some(if camera_device.starts_with("v4l2src") {
        "Infrared camera · live preview unavailable, look at the camera"
    } else {
        "RGB camera"
    }));
    camera_mode.add_css_class("caption");
    camera_mode.add_css_class("dim-label");
    camera_mode.set_halign(gtk4::Align::Start);
    body.append(&camera_mode);

    let resolved_face = Rc::new(RefCell::new(face_name.unwrap_or("default").to_string()));
    let existing_face_names = Rc::new(RefCell::new(HashSet::<String>::new()));
    let face_name_valid = Rc::new(RefCell::new(is_refine));
    let mut face_name_entry: Option<libadwaita::EntryRow> = None;

    if !is_refine {
        let entry = libadwaita::EntryRow::new();
        entry.set_title("Face Name");
        entry.set_text("default");
        let group = libadwaita::PreferencesGroup::new();
        group.add(&entry);
        body.append(&group);
        face_name_entry = Some(entry.clone());

        let rf = resolved_face.clone();
        entry.connect_changed(move |e| {
            let name = e.text().trim().to_string();
            *rf.borrow_mut() = name.clone();
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
    prompt_label.set_visible(false);
    body.append(&prompt_label);

    let progress_label = gtk4::Label::new(None);
    progress_label.add_css_class("dim-label");
    progress_label.set_margin_bottom(2);
    progress_label.set_visible(false);
    body.append(&progress_label);

    let progress = gtk4::ProgressBar::new();
    progress.set_visible(false);
    body.append(&progress);

    let start_btn = gtk4::Button::with_label(if is_refine {
        "Start Update"
    } else {
        "Start Capture"
    });
    start_btn.add_css_class("suggested-action");
    start_btn.add_css_class("pill");
    start_btn.set_halign(gtk4::Align::Center);
    start_btn.set_sensitive(is_refine);
    body.append(&start_btn);

    if let Some(entry) = face_name_entry {
        let start_btn_for_validation = start_btn.clone();
        let existing_face_names = existing_face_names.clone();
        let face_name_valid = face_name_valid.clone();
        entry.connect_changed(move |e| {
            let name = e.text().trim().to_string();
            let valid = !name.is_empty() && !existing_face_names.borrow().contains(&name);
            *face_name_valid.borrow_mut() = valid;
            start_btn_for_validation.set_sensitive(valid);
        });
    }

    let stop_btn = gtk4::Button::with_label("Cancel");
    stop_btn.add_css_class("destructive-action");
    stop_btn.add_css_class("pill");
    stop_btn.set_halign(gtk4::Align::Center);
    stop_btn.set_visible(false);
    body.append(&stop_btn);

    let show_cancel_confirmation = Rc::new(glib::clone!(
        #[strong]
        proxy,
        #[strong]
        feed,
        #[strong]
        on_done,
        #[weak]
        stop_btn,
        move |parent: gtk4::Window| {
            let confirm = libadwaita::MessageDialog::builder()
                .heading(if is_refine {
                    "Cancel Template Update?"
                } else {
                    "Cancel Template Capture?"
                })
                .body("This will discard any partial captures.")
                .transient_for(&parent)
                .build();

            confirm.add_response("resume", "Resume");
            confirm.add_response("discard", "Discard");
            confirm.set_response_appearance("discard", libadwaita::ResponseAppearance::Destructive);

            confirm.connect_response(
                None,
                glib::clone!(
                    #[strong]
                    proxy,
                    #[strong]
                    feed,
                    #[strong]
                    on_done,
                    #[weak]
                    parent,
                    #[weak]
                    stop_btn,
                    move |c, response| {
                        if response == "discard" {
                            stop_btn.set_visible(false);
                            glib::MainContext::default().spawn_local(glib::clone!(
                                #[strong]
                                proxy,
                                async move {
                                    let _ = proxy.enroll_stop().await;
                                    let _ = proxy.release().await;
                                }
                            ));
                            feed.stop();
                            on_done();
                            parent.close();
                        }
                        c.close();
                    }
                ),
            );
            confirm.present();
        }
    ));

    content.append(&body);
    dialog.set_content(Some(&content));

    let username = username.to_string();

    if !is_refine {
        glib::MainContext::default().spawn_local(glib::clone!(
            #[weak]
            start_btn,
            #[strong]
            proxy,
            #[strong]
            username,
            #[strong]
            resolved_face,
            #[strong]
            existing_face_names,
            #[strong]
            face_name_valid,
            async move {
                if let Ok(faces) = proxy.list_faces(&username).await {
                    let mut names = existing_face_names.borrow_mut();
                    names.clear();
                    names.extend(faces.into_iter().map(|(name, _)| name));
                }

                let current_name = resolved_face.borrow().trim().to_string();
                let valid = !current_name.is_empty()
                    && !existing_face_names.borrow().contains(&current_name);
                *face_name_valid.borrow_mut() = valid;
                start_btn.set_sensitive(valid);
            }
        ));
    }

    start_btn.connect_clicked(glib::clone!(
        #[weak]
        stop_btn,
        #[weak]
        prompt_label,
        #[weak]
        progress,
        #[weak]
        progress_label,
        #[strong]
        proxy,
        #[strong]
        resolved_face,
        #[weak]
        dialog,
        #[strong]
        on_done,
        #[strong]
        feed,
        move |btn| {
            btn.set_visible(false);
            stop_btn.set_visible(true);
            prompt_label.set_visible(true);
            progress_label.set_visible(true);
            progress.set_visible(true);
            prompt_label.set_text("Starting enrollment...");
            feed.set_active(true);

            let face_name = resolved_face.borrow().clone();

            glib::MainContext::default().spawn_local(glib::clone!(
                #[strong]
                proxy,
                #[weak]
                progress,
                #[weak]
                progress_label,
                #[weak]
                prompt_label,
                #[weak]
                dialog,
                #[weak]
                stop_btn,
                #[strong]
                on_done,
                #[strong]
                feed,
                async move {
                    let mut enroll_stream = match proxy.receive_enroll_status().await {
                        Ok(s) => s,
                        Err(_) => {
                            prompt_label.set_text("Failed to connect to enrollment stream.");
                            let _ = proxy.release().await;
                            return;
                        }
                    };

                    let mut capture_stream = match proxy.receive_face_status().await {
                        Ok(s) => s,
                        Err(_) => {
                            prompt_label.set_text("Failed to connect to capture stream.");
                            let _ = proxy.release().await;
                            return;
                        }
                    };

                    if proxy.enroll_start(&face_name).await.is_err() {
                        prompt_label.set_text("Daemon failed to start enrollment.");
                        let _ = proxy.release().await;
                        return;
                    }

                    loop {
                        tokio::select! {
                            Some(signal) = enroll_stream.next() => {
                                if let Ok(args) = signal.args() {
                                    let prog = *args.progress();
                                    let max = *args.max();
                                    let raw_msg = *args.msg();
                                    let time_remaining = *args.time_remaining();
                                    let is_done = *args.is_done();

                                    let display_msg = raw_msg.to_string();

                                    if time_remaining > 0.0 {
                                        prompt_label.set_text(&format!("{} [{:.1}s]", display_msg, time_remaining));
                                    } else {
                                        prompt_label.set_text(&display_msg);
                                    }

                                    if max > 0 {
                                        let frac = prog as f64 / max as f64;
                                        progress.set_fraction(frac);
                                        progress_label.set_text(&format!("{}/{}", prog, max));
                                    }

                                    if matches!(raw_msg, EnrollPrompt::DbFailed | EnrollPrompt::Cancelled) {
                                        prompt_label.set_text(raw_msg.as_ref());
                                        stop_btn.set_visible(false);
                                        break;
                                    }

                                    if is_done && raw_msg == EnrollPrompt::Completed {
                                        prompt_label.set_text("✓ Enrollment Complete!");
                                        stop_btn.set_visible(false);
                                        on_done();
                                        glib::timeout_add_local_once(
                                            std::time::Duration::from_millis(1500),
                                            glib::clone!(#[weak] dialog, move || {
                                                dialog.close();
                                            })
                                        );
                                        break;
                                    }

                                    if is_done {
                                        prompt_label.set_text("Enrollment finished without saving.");
                                        stop_btn.set_visible(false);
                                        break;
                                    }
                                }
                            }
                            Some(signal) = capture_stream.next() => {
                                if let Ok(args) = signal.args() {
                                    let status = *args.status();
                                    feed.set_face_status(status);
                                }
                            }
                        }
                    }

                    let _ = proxy.release().await;
                }
            ));
        }
    ));

    stop_btn.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[strong]
        show_cancel_confirmation,
        move |_| {
            show_cancel_confirmation(dialog.upcast());
        }
    ));

    dialog.connect_close_request(glib::clone!(
        #[strong]
        feed,
        #[strong]
        on_done,
        #[strong]
        proxy,
        #[strong]
        show_cancel_confirmation,
        move |dialog| {
            if stop_btn.get_visible() && !prompt_label.text().contains("Complete") {
                show_cancel_confirmation(dialog.clone().upcast());
                glib::Propagation::Stop
            } else {
                glib::MainContext::default().spawn_local(glib::clone!(
                    #[strong]
                    proxy,
                    async move {
                        let _ = proxy.enroll_stop().await;
                        let _ = proxy.release().await;
                    }
                ));
                feed.stop();
                on_done();
                glib::Propagation::Proceed
            }
        }
    ));

    dialog.present();
}
