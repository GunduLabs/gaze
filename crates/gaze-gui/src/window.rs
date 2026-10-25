// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::dialog::capture;
use crate::dialog::config::show_config_dialog;
use gaze_core::dbus::{
    DaemonState, GazeProxy, connect_gaze, dbus_error_message, dbus_is_file_not_found,
    dbus_is_not_activatable, gazed_unit_state_on, load_config_from_daemon,
};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::Duration;

type RefreshCb = Rc<dyn Fn()>;

const AUTH_PROMPT_PRESENTATION_DELAY: Duration = Duration::from_millis(300);

fn load_auth_highlight_css() {
    static AUTH_HIGHLIGHT_CSS: OnceLock<()> = OnceLock::new();

    AUTH_HIGHLIGHT_CSS.get_or_init(|| {
        let provider = gtk4::CssProvider::new();
        provider.load_from_string(
            ".auth-match-highlight {
                background: alpha(@accent_bg_color, 0.35);
                transition: background 220ms ease-in-out;
            }",
        );

        if let Some(display) = gtk4::gdk::Display::default() {
            gtk4::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
}

fn toast_overlay(window: &libadwaita::ApplicationWindow) -> Option<libadwaita::ToastOverlay> {
    window
        .content()
        .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
}

fn add_toast(window: &libadwaita::ApplicationWindow, text: impl AsRef<str>) -> libadwaita::Toast {
    let toast = libadwaita::Toast::new(text.as_ref());
    if let Some(overlay) = toast_overlay(window) {
        overlay.add_toast(toast.clone());
    }
    toast
}

fn add_dbus_error_toast(window: &libadwaita::ApplicationWindow, prefix: &str, err: &zbus::Error) {
    add_toast(window, format!("{}: {}", prefix, dbus_error_message(err)));
}

async fn confirm_face_removal(window: &libadwaita::ApplicationWindow, face_name: &str) -> bool {
    let dialog = libadwaita::MessageDialog::builder()
        .heading(format!("Remove \u{201c}{face_name}\u{201d}?"))
        .body("Gaze will stop recognizing this face. You can enroll it again later.")
        .transient_for(window)
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("remove", "Remove");
    dialog.set_response_appearance("remove", libadwaita::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.choose_future().await == "remove"
}

fn daemon_state_text(state: Option<DaemonState>) -> (&'static str, &'static str, Duration) {
    match state {
        Some(DaemonState::Failed) => (
            "Daemon Failed",
            "The Gaze daemon stopped with an error. Check `journalctl -u gazed -n 50`, then run `sudo systemctl restart gazed`.",
            Duration::from_secs(10),
        ),
        Some(DaemonState::Disabled) => (
            "Daemon Disabled",
            "The Gaze daemon is turned off. Turn it on with `sudo systemctl enable --now gazed`.",
            Duration::from_secs(10),
        ),
        Some(DaemonState::Stopped) => (
            "Daemon Stopped",
            "The Gaze daemon is not running. Start it with `sudo systemctl start gazed`.",
            Duration::from_secs(10),
        ),
        Some(DaemonState::NotInstalled) => (
            "Daemon Not Installed",
            "The gazed service is missing. Install the gaze package for your distribution.",
            Duration::from_secs(10),
        ),
        Some(DaemonState::Starting) | None => (
            "Daemon Starting",
            "The Gaze daemon is starting up. Its first start downloads face models, which can take a few minutes.",
            Duration::from_secs(3),
        ),
    }
}

fn show_daemon_pending_toast(window: &libadwaita::ApplicationWindow) {
    add_toast(window, "Connecting to the Gaze daemon…");
}

async fn begin_face_capture(
    window: &libadwaita::ApplicationWindow,
    username: &str,
    face_name: Option<&str>,
    proxy: &Rc<GazeProxy<'static>>,
    refresh: &Rc<RefCell<Option<RefreshCb>>>,
) {
    // Start the Polkit request first. If it needs interaction, give the agent a moment to
    // present its prompt, then warm the camera while the user authenticates.
    let mut authorization = Box::pin(proxy.authorize_enrollment());
    let authorization_done = match futures::future::select(
        authorization.as_mut(),
        glib::timeout_future(AUTH_PROMPT_PRESENTATION_DELAY),
    )
    .await
    {
        futures::future::Either::Left((Ok(()), _)) => true,
        futures::future::Either::Left((Err(err), _)) => {
            add_toast(window, format!("Face enrollment: {err}"));
            return;
        }
        futures::future::Either::Right(_) => false,
    };

    let camera = match load_config_from_daemon(proxy).await {
        Ok(cfg) => capture::CameraSetup::from_config(&cfg.cameras),
        Err(_) => capture::CameraSetup::fallback(),
    };
    let feed = match capture::prepare_camera_feed(&camera) {
        Ok(feed) => feed,
        Err(err) => {
            add_toast(window, format!("Failed to start camera: {err}"));
            return;
        }
    };

    if !authorization_done && let Err(err) = authorization.await {
        feed.stop();
        add_toast(window, format!("Face enrollment: {err}"));
        return;
    }
    if let Err(err) = proxy.claim(username).await {
        feed.stop();
        add_dbus_error_toast(window, "Failed to claim device", &err);
        return;
    }

    capture::show_capture_dialog(
        window,
        username,
        face_name,
        proxy,
        &camera,
        feed,
        glib::clone!(
            #[strong]
            refresh,
            move || {
                if let Some(f) = refresh.borrow().as_ref() {
                    f();
                }
            }
        ),
    );
}

/// Returns green when the profile has captures for this spectrum, amber when a camera is
/// configured but the profile has no captures, and unlit when no camera is configured.
fn spectrum_badge_class(enrolled: bool, configured: bool) -> &'static str {
    match (enrolled, configured) {
        (true, _) => "badge-success",
        (false, true) => "badge-warning",
        (false, false) => "badge-muted",
    }
}

pub fn build_window(app: &libadwaita::Application, username: &str) {
    load_auth_highlight_css();

    let username = Rc::new(username.to_string());

    let window = libadwaita::ApplicationWindow::builder()
        .application(app)
        .title("Gaze")
        .default_width(460)
        .default_height(500)
        .build();

    let main_box = gtk4::Box::new(gtk4::Orientation::Vertical, 0);

    let header = libadwaita::HeaderBar::new();
    let title = libadwaita::WindowTitle::new("Gaze", &format!("User: {username}"));
    header.set_title_widget(Some(&title));

    let add_btn = gtk4::Button::from_icon_name("list-add-symbolic");
    add_btn.set_tooltip_text(Some("Enroll a new face"));

    let test_btn = gtk4::Button::from_icon_name("media-playback-start-symbolic");
    test_btn.set_tooltip_text(Some("Test face authentication"));

    let config_btn = gtk4::Button::from_icon_name("emblem-system-symbolic");
    config_btn.set_tooltip_text(Some("Configure Gaze"));

    header.pack_end(&add_btn);
    header.pack_end(&test_btn);
    header.pack_end(&config_btn);

    main_box.append(&header);

    let scroll = gtk4::ScrolledWindow::new();
    scroll.set_vexpand(true);

    let clamp = libadwaita::Clamp::new();
    clamp.set_maximum_size(600);

    let content = gtk4::Box::new(gtk4::Orientation::Vertical, 16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.set_margin_top(16);
    content.set_margin_bottom(16);

    let face_group = libadwaita::PreferencesGroup::new();
    face_group.set_title("Enrolled Faces");
    face_group.set_description(Some(&format!("Faces enrolled for {username}")));

    let face_list = gtk4::ListBox::new();
    face_list.add_css_class("boxed-list");
    face_list.set_selection_mode(gtk4::SelectionMode::None);
    face_group.add(&face_list);

    content.append(&face_group);

    let status_page = libadwaita::StatusPage::new();
    status_page.set_icon_name(Some("contact-new-symbolic"));
    status_page.set_title("No Faces Enrolled");
    status_page.set_description(Some("Loading from the Gaze daemon…"));
    status_page.set_visible(true);
    face_list.set_visible(false);
    content.append(&status_page);

    clamp.set_child(Some(&content));
    scroll.set_child(Some(&clamp));
    main_box.append(&scroll);

    let toast_overlay = libadwaita::ToastOverlay::new();
    toast_overlay.set_child(Some(&main_box));
    window.set_content(Some(&toast_overlay));
    window.present();

    config_btn.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |_| {
            if let Some(overlay) = window
                .content()
                .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
            {
                show_config_dialog(&window, &overlay)
            }
        }
    ));

    let proxy_cell: Rc<RefCell<Option<Rc<GazeProxy>>>> = Rc::new(RefCell::new(None));
    let refresh: Rc<RefCell<Option<RefreshCb>>> = Rc::new(RefCell::new(None));
    let last_toast: Rc<RefCell<Option<libadwaita::Toast>>> = Rc::new(RefCell::new(None));

    test_btn.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        proxy_cell,
        #[weak(rename_to = face_list_weak)]
        face_list,
        #[strong]
        username,
        #[strong]
        last_toast,
        move |btn| {
            let Some(proxy) = proxy_cell.borrow().clone() else {
                show_daemon_pending_toast(&window);
                return;
            };
            if let Some(prev) = last_toast.borrow_mut().take() {
                prev.dismiss();
            }
            btn.set_sensitive(false);
            glib::MainContext::default().spawn_local(glib::clone!(
                #[weak]
                window,
                #[strong]
                username,
                #[weak]
                btn,
                #[strong]
                proxy,
                #[strong]
                face_list_weak,
                #[strong]
                last_toast,
                async move {
                    use futures::StreamExt;

                    if proxy.claim(&username).await.is_err() {
                        add_toast(&window, "Failed to claim device");
                        btn.set_sensitive(true);
                        return;
                    }
                    let mut stream = match proxy.receive_verify_status().await {
                        Ok(stream) => stream,
                        Err(_) => {
                            add_toast(&window, "Daemon error starting verification");
                            let _ = proxy.release().await;
                            btn.set_sensitive(true);
                            return;
                        }
                    };

                    if proxy.verify_start("any").await.is_err() {
                        add_toast(&window, "Daemon error starting verification");
                        let _ = proxy.release().await;
                        btn.set_sensitive(true);
                        return;
                    }

                    let mut text = "Face not recognized".to_string();
                    let mut matched_face: Option<String> = None;

                    // Without a deadline of its own the button stays stuck for as long as the
                    // daemon withholds a verdict.
                    let deadline = glib::timeout_future(gaze_core::dbus::VERIFY_CLIENT_TIMEOUT);
                    let mut deadline = std::pin::pin!(deadline);
                    loop {
                        match futures::future::select(stream.next(), deadline.as_mut()).await {
                            futures::future::Either::Left((Some(signal), _)) => {
                                let Ok(args) = signal.args() else { continue };
                                let res = *args.result();
                                if res == gaze_core::dbus::VerifyResult::VerifyMatch {
                                    text = "Face recognized".to_string();
                                    let faces = args.faces();
                                    matched_face = faces
                                        .iter()
                                        .find(|(_, _, _, rgb_p, _, _, ir_p)| *rgb_p || *ir_p)
                                        .map(|(n, _, _, _, _, _, _)| n.clone());
                                } else {
                                    text = "Face not recognized".to_string();
                                }
                                break;
                            }
                            futures::future::Either::Left((None, _)) => break,
                            futures::future::Either::Right(_) => {
                                let _ = proxy.verify_stop().await;
                                text = "No face seen before the test timed out".to_string();
                                break;
                            }
                        }
                    }

                    let _ = proxy.release().await;

                    if let Some(face_name) = matched_face {
                        let list = face_list_weak;
                        let mut child = list.first_child();
                        while let Some(w) = child {
                            if let Ok(row) = w.clone().downcast::<libadwaita::ActionRow>() {
                                let title: gtk4::glib::GString = row.title();
                                let is_match = title.as_str() == face_name.as_str();
                                if is_match {
                                    row.add_css_class("auth-match-highlight");
                                    let r = row;
                                    glib::timeout_add_local_once(
                                        std::time::Duration::from_secs(2),
                                        move || {
                                            r.remove_css_class("auth-match-highlight");
                                        },
                                    );
                                    break;
                                }
                            }
                            child = w.next_sibling();
                        }
                    }

                    let toast = add_toast(&window, &text);
                    *last_toast.borrow_mut() = Some(toast);
                    btn.set_sensitive(true);
                }
            ));
        }
    ));

    add_btn.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        username,
        #[strong]
        refresh,
        #[strong]
        proxy_cell,
        move |_| {
            let Some(proxy) = proxy_cell.borrow().clone() else {
                show_daemon_pending_toast(&window);
                return;
            };
            glib::MainContext::default().spawn_local(glib::clone!(
                #[weak]
                window,
                #[strong]
                username,
                #[strong]
                refresh,
                #[strong]
                proxy,
                async move {
                    begin_face_capture(&window, &username, None, &proxy, &refresh).await;
                }
            ));
        }
    ));

    glib::MainContext::default().spawn_local(glib::clone!(
        #[weak]
        window,
        #[weak]
        face_list,
        #[weak]
        status_page,
        #[strong]
        username,
        #[strong]
        proxy_cell,
        #[strong]
        refresh,
        async move {
            let Ok(proxy) = connect_gaze().await else {
                tracing::error!("Failed to connect to Gaze daemon");
                status_page.set_description(Some("Failed to connect to Gaze daemon"));
                return;
            };

            let proxy = Rc::new(proxy);
            *proxy_cell.borrow_mut() = Some(proxy.clone());

            *refresh.borrow_mut() = Some(Rc::new(glib::clone!(
                #[weak]
                face_list,
                #[weak]
                status_page,
                #[strong]
                username,
                #[weak]
                window,
                #[strong]
                refresh,
                #[strong]
                proxy,
                move || {
                    glib::MainContext::default().spawn_local(glib::clone!(
                        #[weak]
                        face_list,
                        #[weak]
                        status_page,
                        #[strong]
                        username,
                        #[weak]
                        window,
                        #[strong]
                        refresh,
                        #[strong]
                        proxy,
                        async move {
                            let faces = match proxy.list_faces(&username).await {
                                Ok(faces) => faces,
                                Err(err) => {
                                    if dbus_is_file_not_found(&err) {
                                        Vec::new()
                                    } else if dbus_is_not_activatable(&err)
                                        && !gaze_core::cpu::supports_inference()
                                    {
                                        // This CPU cannot run `gazed`, so retrying the request will not
                                        // help.
                                        status_page.set_title("Unsupported CPU");
                                        status_page.set_description(Some(
                                            gaze_core::cpu::UNSUPPORTED_CPU_MESSAGE,
                                        ));
                                        status_page.set_visible(true);
                                        face_list.set_visible(false);
                                        return;
                                    } else if dbus_is_not_activatable(&err) {
                                        let state = gazed_unit_state_on(proxy.inner().connection())
                                            .await
                                            .ok();
                                        let (title, description, retry) = daemon_state_text(state);
                                        status_page.set_title(title);
                                        status_page.set_description(Some(description));
                                        status_page.set_visible(true);
                                        face_list.set_visible(false);
                                        glib::timeout_add_local_once(
                                            retry,
                                            glib::clone!(
                                                #[strong]
                                                refresh,
                                                move || {
                                                    if let Some(f) = refresh.borrow().as_ref() {
                                                        f();
                                                    }
                                                }
                                            ),
                                        );
                                        return;
                                    } else {
                                        add_dbus_error_toast(&window, "Failed to load faces", &err);
                                        Vec::new()
                                    }
                                }
                            };

                            while let Some(child) = face_list.first_child() {
                                face_list.remove(&child);
                            }

                            if faces.is_empty() {
                                status_page.set_title("No Faces Enrolled");
                                status_page.set_description(Some("Enroll a face to get started"));
                                status_page.set_visible(true);
                                face_list.set_visible(false);
                            } else {
                                status_page.set_visible(false);
                                face_list.set_visible(true);

                                // A spectrum with no camera configured is not a gap
                                // in the profile, so its badge reads unlit, not failed.
                                let (rgb_configured, ir_configured) =
                                    match load_config_from_daemon(&proxy).await {
                                        Ok(cfg) => (
                                            !cfg.cameras.rgb.trim().is_empty(),
                                            !cfg.cameras.ir.trim().is_empty(),
                                        ),
                                        Err(_) => (true, true),
                                    };

                                let existing_face_names: Rc<std::collections::HashSet<String>> =
                                    Rc::new(
                                        faces
                                            .iter()
                                            .map(|(name, _, _, _): &(String, u32, bool, bool)| {
                                                name.clone()
                                            })
                                            .collect(),
                                    );

                                for (face_name, count, has_rgb, has_ir) in faces {
                                    let row = libadwaita::ActionRow::new();
                                    row.set_title(&face_name);
                                    row.set_subtitle(&format!(
                                        "{} capture{}",
                                        count,
                                        if count == 1 { "" } else { "s" }
                                    ));

                                    let rgb_badge = gtk4::Label::new(Some("RGB"));
                                    rgb_badge.set_valign(gtk4::Align::Center);
                                    rgb_badge.add_css_class(spectrum_badge_class(
                                        has_rgb,
                                        rgb_configured,
                                    ));

                                    let ir_badge = gtk4::Label::new(Some("IR"));
                                    ir_badge.set_valign(gtk4::Align::Center);
                                    ir_badge
                                        .add_css_class(spectrum_badge_class(has_ir, ir_configured));

                                    let badge_box =
                                        gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
                                    badge_box.set_valign(gtk4::Align::Center);
                                    badge_box.append(&rgb_badge);
                                    badge_box.append(&ir_badge);

                                    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
                                    btn_box.set_valign(gtk4::Align::Center);

                                    let rename_btn =
                                        gtk4::Button::from_icon_name("document-edit-symbolic");
                                    rename_btn.add_css_class("flat");
                                    rename_btn.set_tooltip_text(Some("Rename"));
                                    let refine_btn =
                                        gtk4::Button::from_icon_name("camera-photo-symbolic");
                                    refine_btn.add_css_class("flat");
                                    refine_btn.set_tooltip_text(Some("Improve this face"));
                                    let delete_btn =
                                        gtk4::Button::from_icon_name("user-trash-symbolic");
                                    delete_btn.add_css_class("flat");
                                    delete_btn.set_tooltip_text(Some("Delete"));

                                    btn_box.append(&rename_btn);
                                    btn_box.append(&refine_btn);
                                    btn_box.append(&delete_btn);
                                    row.add_suffix(&badge_box);
                                    row.add_suffix(&btn_box);

                                    rename_btn.connect_clicked(glib::clone!(
                                        #[weak]
                                        rename_btn,
                                        #[weak]
                                        window,
                                        #[strong]
                                        username,
                                        #[strong]
                                        face_name,
                                        #[strong]
                                        refresh,
                                        #[strong]
                                        existing_face_names,
                                        #[strong]
                                        proxy,
                                        move |_| {
                                            let popover = gtk4::Popover::new();
                                            popover.set_has_arrow(true);
                                            popover.set_autohide(true);
                                            popover.set_parent(&rename_btn);

                                            let body =
                                                gtk4::Box::new(gtk4::Orientation::Vertical, 8);
                                            body.set_margin_start(10);
                                            body.set_margin_end(10);
                                            body.set_margin_top(10);
                                            body.set_margin_bottom(10);

                                            let entry = gtk4::Entry::new();
                                            entry.set_placeholder_text(Some("New face name"));
                                            entry.set_text(&face_name);
                                            body.append(&entry);

                                            let button_row =
                                                gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                                            button_row.set_halign(gtk4::Align::End);

                                            let cancel_btn = gtk4::Button::with_label("Cancel");
                                            let rename_confirm_btn =
                                                gtk4::Button::with_label("Rename");
                                            rename_confirm_btn.add_css_class("suggested-action");
                                            rename_confirm_btn.set_sensitive(false);

                                            button_row.append(&cancel_btn);
                                            button_row.append(&rename_confirm_btn);
                                            body.append(&button_row);

                                            popover.set_child(Some(&body));

                                            entry.connect_changed(glib::clone!(
                                                #[weak]
                                                rename_confirm_btn,
                                                #[strong]
                                                face_name,
                                                #[strong]
                                                existing_face_names,
                                                move |e| {
                                                    let new_name = e.text().trim().to_string();
                                                    let valid = !new_name.is_empty()
                                                        && new_name != face_name
                                                        && !existing_face_names.contains(&new_name);
                                                    rename_confirm_btn.set_sensitive(valid);
                                                }
                                            ));

                                            cancel_btn.connect_clicked(glib::clone!(
                                                #[weak]
                                                popover,
                                                move |_| {
                                                    popover.popdown();
                                                }
                                            ));

                                            rename_confirm_btn.connect_clicked(glib::clone!(
                                                #[weak]
                                                window,
                                                #[weak]
                                                popover,
                                                #[strong]
                                                username,
                                                #[strong]
                                                face_name,
                                                #[strong]
                                                refresh,
                                                #[strong]
                                                proxy,
                                                move |_| {
                                                    let new_name = entry.text().trim().to_string();
                                                    if new_name.is_empty() || new_name == face_name
                                                    {
                                                        popover.popdown();
                                                        return;
                                                    }

                                                    glib::MainContext::default().spawn_local(
                                                        glib::clone!(
                                                            #[weak]
                                                            window,
                                                            #[strong]
                                                            username,
                                                            #[strong]
                                                            face_name,
                                                            #[strong]
                                                            new_name,
                                                            #[strong]
                                                            refresh,
                                                            #[strong]
                                                            proxy,
                                                            async move {
                                                                if let Err(err) = proxy
                                                                    .rename_face(
                                                                        &username, &face_name,
                                                                        &new_name,
                                                                    )
                                                                    .await
                                                                {
                                                                    add_dbus_error_toast(
                                                                        &window,
                                                                        "Failed to rename face",
                                                                        &err,
                                                                    );
                                                                } else {
                                                                    if let Some(f) =
                                                                        refresh.borrow().as_ref()
                                                                    {
                                                                        f();
                                                                    }

                                                                    let text = format!(
                                                                        "Renamed '{}' to '{}'",
                                                                        face_name, new_name
                                                                    );
                                                                    add_toast(&window, text);
                                                                }
                                                            }
                                                        ),
                                                    );

                                                    popover.popdown();
                                                }
                                            ));

                                            popover.popup();
                                        }
                                    ));
                                    refine_btn.connect_clicked(glib::clone!(
                                        #[weak]
                                        window,
                                        #[strong]
                                        username,
                                        #[strong]
                                        face_name,
                                        #[strong]
                                        refresh,
                                        #[strong]
                                        proxy,
                                        move |_| {
                                            glib::MainContext::default().spawn_local(glib::clone!(
                                                #[weak]
                                                window,
                                                #[strong]
                                                username,
                                                #[strong]
                                                face_name,
                                                #[strong]
                                                refresh,
                                                #[strong]
                                                proxy,
                                                async move {
                                                    begin_face_capture(
                                                        &window,
                                                        &username,
                                                        Some(&face_name),
                                                        &proxy,
                                                        &refresh,
                                                    )
                                                    .await;
                                                }
                                            ));
                                        }
                                    ));

                                    delete_btn.connect_clicked(glib::clone!(
                                        #[weak]
                                        window,
                                        #[strong]
                                        username,
                                        #[strong]
                                        face_name,
                                        #[strong]
                                        refresh,
                                        #[strong]
                                        proxy,
                                        move |_| {
                                            glib::MainContext::default().spawn_local(glib::clone!(
                                                #[weak]
                                                window,
                                                #[strong]
                                                username,
                                                #[strong]
                                                face_name,
                                                #[strong]
                                                refresh,
                                                #[strong]
                                                proxy,
                                                async move {
                                                    if !confirm_face_removal(&window, &face_name)
                                                        .await
                                                    {
                                                        return;
                                                    }
                                                    if let Err(err) = proxy
                                                        .delete_face(&username, &face_name)
                                                        .await
                                                    {
                                                        add_dbus_error_toast(
                                                            &window,
                                                            "Failed to remove face",
                                                            &err,
                                                        );
                                                    }
                                                    if let Some(f) = refresh.borrow().as_ref() {
                                                        f();
                                                    }
                                                }
                                            ));
                                        }
                                    ));

                                    face_list.append(&row);
                                }
                            }
                        }
                    ));
                }
            )));

            if let Some(f) = refresh.borrow().as_ref() {
                f();
            }
        }
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_badges_distinguish_enrolled_configured_and_absent() {
        assert_eq!(spectrum_badge_class(true, true), "badge-success");
        assert_eq!(spectrum_badge_class(true, false), "badge-success");
        assert_eq!(spectrum_badge_class(false, true), "badge-warning");
        assert_eq!(spectrum_badge_class(false, false), "badge-muted");
    }
}
