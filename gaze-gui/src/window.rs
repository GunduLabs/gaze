use crate::capture_dialog;
use gaze_core::capture::{init_camera_and_checker, wait_for_capture};
use gaze_core::config::{Config, SecurityLevel};
use gaze_core::dbus::{
    AuthProxy, apply_config_to_daemon, dbus_error_message, dbus_is_file_not_found,
    load_config_from_daemon,
};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;
use zbus::Connection;

type RefreshCb = Rc<dyn Fn()>;

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

#[allow(deprecated)]
fn show_config_dialog(parent: &libadwaita::ApplicationWindow, overlay: &libadwaita::ToastOverlay) {
    let config = Rc::new(RefCell::new(Config::default()));

    let dialog = gtk4::Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Configuration")
        .default_width(560)
        .default_height(420)
        .build();
    dialog.add_button("Cancel", gtk4::ResponseType::Cancel);
    dialog.add_button("Save", gtk4::ResponseType::Accept);

    let content = dialog.content_area();
    content.set_spacing(12);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.set_margin_top(16);
    content.set_margin_bottom(16);

    let grid = gtk4::Grid::builder()
        .column_spacing(12)
        .row_spacing(12)
        .hexpand(true)
        .build();

    let security_label = gtk4::Label::new(Some("Security level"));
    security_label.set_halign(gtk4::Align::Start);
    let security_combo = gtk4::ComboBoxText::new();
    security_combo.append(Some("low"), "low");
    security_combo.append(Some("medium"), "medium");
    security_combo.append(Some("high"), "high");
    security_combo.append(Some("maximum"), "maximum");
    security_combo.append(Some("custom"), "custom");
    security_combo.set_active_id(Some(config.borrow().security.as_name()));

    let custom_detector_label = gtk4::Label::new(Some("Custom detector model"));
    custom_detector_label.set_halign(gtk4::Align::Start);
    let custom_detector_entry = gtk4::Entry::new();
    custom_detector_entry.set_hexpand(true);

    let custom_recognizer_label = gtk4::Label::new(Some("Custom recognizer model"));
    custom_recognizer_label.set_halign(gtk4::Align::Start);
    let custom_recognizer_entry = gtk4::Entry::new();
    custom_recognizer_entry.set_hexpand(true);

    let custom_threshold_label = gtk4::Label::new(Some("Custom threshold"));
    custom_threshold_label.set_halign(gtk4::Align::Start);
    let custom_threshold_spin = gtk4::SpinButton::with_range(0.0, 1.0, 0.01);
    custom_threshold_spin.set_digits(3);

    let custom_grid = gtk4::Grid::builder()
        .column_spacing(12)
        .row_spacing(10)
        .hexpand(true)
        .build();
    custom_grid.attach(&custom_detector_label, 0, 0, 1, 1);
    custom_grid.attach(&custom_detector_entry, 1, 0, 1, 1);
    custom_grid.attach(&custom_recognizer_label, 0, 1, 1, 1);
    custom_grid.attach(&custom_recognizer_entry, 1, 1, 1, 1);
    custom_grid.attach(&custom_threshold_label, 0, 2, 1, 1);
    custom_grid.attach(&custom_threshold_spin, 1, 2, 1, 1);

    let custom_frame = gtk4::Frame::new(Some("Custom level settings"));
    custom_frame.set_child(Some(&custom_grid));

    {
        let cfg = config.borrow();
        let (detector, recognizer, threshold) = match &cfg.security {
            SecurityLevel::Custom {
                detector,
                recognizer,
                threshold,
            } => (detector.clone(), recognizer.clone(), *threshold),
            _ => (
                cfg.security.detector().to_string(),
                cfg.security.recognizer().to_string(),
                cfg.security.threshold(),
            ),
        };
        custom_detector_entry.set_text(&detector);
        custom_recognizer_entry.set_text(&recognizer);
        custom_threshold_spin.set_value(threshold as f64);
    }

    let camera_label = gtk4::Label::new(Some("RGB camera"));
    camera_label.set_halign(gtk4::Align::Start);
    let camera_entry = gtk4::Entry::new();
    camera_entry.set_hexpand(true);
    camera_entry.set_text(&config.borrow().cameras.rgb);

    let captures_label = gtk4::Label::new(Some("Max captures per face"));
    captures_label.set_halign(gtk4::Align::Start);
    let captures_spin = gtk4::SpinButton::with_range(1.0, 64.0, 1.0);
    captures_spin.set_value(config.borrow().enrollment.max_captures_per_face as f64);

    grid.attach(&security_label, 0, 0, 1, 1);
    grid.attach(&security_combo, 1, 0, 1, 1);
    grid.attach(&camera_label, 0, 1, 1, 1);
    grid.attach(&camera_entry, 1, 1, 1, 1);
    grid.attach(&captures_label, 0, 2, 1, 1);
    grid.attach(&captures_spin, 1, 2, 1, 1);
    grid.attach(&custom_frame, 0, 3, 2, 1);

    let update_custom_visibility: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[weak]
        security_combo,
        #[weak]
        custom_frame,
        move || {
            let is_custom = security_combo.active_id().as_deref() == Some("custom");
            custom_frame.set_visible(is_custom);
        }
    ));
    security_combo.connect_changed(glib::clone!(
        #[strong]
        update_custom_visibility,
        move |_| {
            update_custom_visibility();
        }
    ));
    update_custom_visibility();

    content.append(&grid);

    glib::MainContext::default().spawn_local(glib::clone!(
        #[weak]
        security_combo,
        #[weak]
        camera_entry,
        #[weak]
        captures_spin,
        #[weak]
        custom_detector_entry,
        #[weak]
        custom_recognizer_entry,
        #[weak]
        custom_threshold_spin,
        #[strong]
        update_custom_visibility,
        #[weak]
        overlay,
        #[strong]
        config,
        async move {
            let load_result = async {
                let conn = Connection::system().await?;
                let proxy = AuthProxy::new(&conn).await?;
                load_config_from_daemon(&proxy).await
            }
            .await;

            match load_result {
                Ok(cfg) => {
                    security_combo.set_active_id(Some(cfg.security.as_name()));
                    camera_entry.set_text(&cfg.cameras.rgb);
                    captures_spin.set_value(cfg.enrollment.max_captures_per_face as f64);
                    let (detector, recognizer, threshold) = match &cfg.security {
                        SecurityLevel::Custom {
                            detector,
                            recognizer,
                            threshold,
                        } => (detector.clone(), recognizer.clone(), *threshold),
                        _ => (
                            cfg.security.detector().to_string(),
                            cfg.security.recognizer().to_string(),
                            cfg.security.threshold(),
                        ),
                    };
                    custom_detector_entry.set_text(&detector);
                    custom_recognizer_entry.set_text(&recognizer);
                    custom_threshold_spin.set_value(threshold as f64);
                    *config.borrow_mut() = cfg;
                    update_custom_visibility();
                }
                Err(err) => {
                    overlay.add_toast(libadwaita::Toast::new(&format!(
                        "Failed to load daemon config: {}",
                        err
                    )));
                }
            }
        }
    ));

    dialog.connect_response(glib::clone!(
        #[weak]
        dialog,
        #[weak]
        overlay,
        #[weak]
        security_combo,
        #[weak]
        camera_entry,
        #[weak]
        captures_spin,
        #[weak]
        custom_detector_entry,
        #[weak]
        custom_recognizer_entry,
        #[weak]
        custom_threshold_spin,
        #[strong]
        config,
        move |_, response| {
            if response != gtk4::ResponseType::Accept {
                dialog.close();
                return;
            }

            let mut cfg = config.borrow_mut();
            cfg.security = match security_combo.active_id().as_deref() {
                Some("low") => SecurityLevel::Low,
                Some("medium") => SecurityLevel::Medium,
                Some("high") => SecurityLevel::High,
                Some("maximum") => SecurityLevel::Maximum,
                Some("custom") => SecurityLevel::Custom {
                    detector: custom_detector_entry.text().to_string(),
                    recognizer: custom_recognizer_entry.text().to_string(),
                    threshold: custom_threshold_spin.value() as f32,
                },
                _ => SecurityLevel::Medium,
            };
            cfg.cameras.rgb = camera_entry.text().to_string();
            cfg.enrollment.max_captures_per_face = captures_spin.value() as usize;

            let cfg_to_apply = cfg.clone();
            drop(cfg);

            glib::MainContext::default().spawn_local(glib::clone!(
                #[weak]
                overlay,
                #[strong]
                cfg_to_apply,
                async move {
                    let apply_result = async {
                        let conn = Connection::system().await?;
                        let proxy = AuthProxy::new(&conn).await?;
                        apply_config_to_daemon(&proxy, &cfg_to_apply).await
                    }
                    .await;

                    match apply_result {
                        Ok(_) => overlay.add_toast(libadwaita::Toast::new(
                            "Configuration saved. Daemon will restart to apply changes.",
                        )),
                        Err(err) => overlay.add_toast(libadwaita::Toast::new(&format!(
                            "Failed to apply daemon config: {}",
                            err
                        ))),
                    }
                }
            ));

            dialog.close();
        }
    ));

    dialog.present();
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
    let title = libadwaita::WindowTitle::new("Gaze", &format!("User: {}", username));
    header.set_title_widget(Some(&title));

    let add_btn = gtk4::Button::from_icon_name("list-add-symbolic");
    add_btn.set_tooltip_text(Some("Add new face"));

    let test_btn = gtk4::Button::from_icon_name("media-playback-start-symbolic");
    test_btn.set_tooltip_text(Some("Test Authentication"));

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
    face_group.set_description(Some("Your registered face profiles"));

    let face_list = gtk4::ListBox::new();
    face_list.add_css_class("boxed-list");
    face_list.set_selection_mode(gtk4::SelectionMode::None);
    face_group.add(&face_list);

    content.append(&face_group);

    let status_page = libadwaita::StatusPage::new();
    status_page.set_icon_name(Some("contact-new-symbolic"));
    status_page.set_title("No Faces Enrolled");
    status_page.set_description(Some("Loading from daemon..."));
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

    glib::MainContext::default().spawn_local(glib::clone!(
        #[weak]
        window,
        #[weak]
        face_list,
        #[weak]
        status_page,
        #[weak]
        add_btn,
        #[weak]
        test_btn,
        #[strong]
        username,
        async move {
            let Ok(conn) = Connection::system().await else {
                tracing::error!("Failed to connect to system DBus");
                status_page.set_description(Some("Failed to connect to system DBus"));
                return;
            };

            let Ok(proxy) = AuthProxy::new(&conn).await else {
                tracing::error!("Failed to create AuthProxy");
                status_page.set_description(Some("Failed to create AuthProxy"));
                return;
            };

            let proxy = Rc::new(proxy);

            let refresh: Rc<RefCell<Option<RefreshCb>>> = Rc::new(RefCell::new(None));

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
                                    } else {
                                        let toast = libadwaita::Toast::new(&format!(
                                            "Failed to load faces: {}",
                                            dbus_error_message(&err)
                                        ));
                                        if let Some(overlay) = window
                                            .content()
                                            .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                                        {
                                            overlay.add_toast(toast);
                                        }
                                        Vec::new()
                                    }
                                }
                            };

                            while let Some(child) = face_list.first_child() {
                                face_list.remove(&child);
                            }

                            if faces.is_empty() {
                                status_page.set_title("No Faces Enrolled");
                                status_page.set_description(Some("Press + to add your first face"));
                                status_page.set_visible(true);
                                face_list.set_visible(false);
                            } else {
                                status_page.set_visible(false);
                                face_list.set_visible(true);

                                let existing_face_names: Rc<std::collections::HashSet<String>> =
                                    Rc::new(faces.iter().map(|(name, _)| name.clone()).collect());

                                for (face_name, count) in faces {
                                    let row = libadwaita::ActionRow::new();
                                    row.set_title(&face_name);
                                    row.set_subtitle(&format!(
                                        "{} capture{}",
                                        count,
                                        if count == 1 { "" } else { "s" }
                                    ));

                                    let btn_box = gtk4::Box::new(gtk4::Orientation::Horizontal, 4);
                                    btn_box.set_valign(gtk4::Align::Center);

                                    let rename_btn =
                                        gtk4::Button::from_icon_name("document-edit-symbolic");
                                    rename_btn.add_css_class("flat");
                                    let refine_btn =
                                        gtk4::Button::from_icon_name("view-refresh-symbolic");
                                    refine_btn.add_css_class("flat");
                                    let delete_btn =
                                        gtk4::Button::from_icon_name("user-trash-symbolic");
                                    delete_btn.add_css_class("flat");

                                    btn_box.append(&rename_btn);
                                    btn_box.append(&refine_btn);
                                    btn_box.append(&delete_btn);
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

                                            let body = gtk4::Box::new(gtk4::Orientation::Vertical, 8);
                                            body.set_margin_start(10);
                                            body.set_margin_end(10);
                                            body.set_margin_top(10);
                                            body.set_margin_bottom(10);

                                            let entry = gtk4::Entry::new();
                                            entry.set_placeholder_text(Some("New face name"));
                                            entry.set_text(&face_name);
                                            body.append(&entry);

                                            let button_row = gtk4::Box::new(gtk4::Orientation::Horizontal, 8);
                                            button_row.set_halign(gtk4::Align::End);

                                            let cancel_btn = gtk4::Button::with_label("Cancel");
                                            let rename_confirm_btn = gtk4::Button::with_label("Rename");
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
                                                    if new_name.is_empty() || new_name == face_name {
                                                        popover.popdown();
                                                        return;
                                                    }

                                                    glib::MainContext::default().spawn_local(glib::clone!(
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
                                                            if let Err(err) = proxy.rename_face(
                                                                &username,
                                                                &face_name,
                                                                &new_name,
                                                            ).await {
                                                                let toast = libadwaita::Toast::new(&format!(
                                                                    "Failed to rename face: {}",
                                                                    dbus_error_message(&err)
                                                                ));
                                                                if let Some(overlay) = window
                                                                    .content()
                                                                    .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                                                                {
                                                                    overlay.add_toast(toast);
                                                                }
                                                            } else {
                                                                if let Some(f) = refresh.borrow().as_ref() {
                                                                    f();
                                                                }

                                                                let text = format!(
                                                                    "Renamed '{}' to '{}'",
                                                                    face_name,
                                                                    new_name
                                                                );
                                                                let toast = libadwaita::Toast::new(&text);
                                                                if let Some(overlay) = window
                                                                    .content()
                                                                    .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                                                                {
                                                                    overlay.add_toast(toast);
                                                                }
                                                            }
                                                        }
                                                    ));

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
                                            capture_dialog::show_capture_dialog(
                                                &window,
                                                &username,
                                                Some(&face_name),
                                                &proxy,
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
                                                    if let Err(err) = proxy
                                                        .remove_face(&username, &face_name)
                                                        .await
                                                    {
                                                        let toast = libadwaita::Toast::new(&format!(
                                                            "Failed to remove face: {}",
                                                            dbus_error_message(&err)
                                                        ));
                                                        if let Some(overlay) = window
                                                            .content()
                                                            .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                                                        {
                                                            overlay.add_toast(toast);
                                                        }
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

            let last_toast: Rc<RefCell<Option<libadwaita::Toast>>> = Rc::new(RefCell::new(None));

            test_btn.connect_clicked(glib::clone!(
                #[weak]
                window,
                #[strong]
                username,
                #[weak]
                face_list,
                #[strong]
                proxy,
                #[strong]
                last_toast,
                move |btn| {
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
                        face_list,
                        #[weak]
                        btn,
                        #[strong]
                        proxy,
                        #[strong]
                        last_toast,
                        async move {
                            let result = (|| -> anyhow::Result<(Vec<u8>, u32, u32)> {
                                let config = glib::MainContext::default()
                                    .block_on(load_config_from_daemon(&proxy))?;
                                let (mut cam, mut checker) =
                                    init_camera_and_checker(&config.cameras.rgb)?;
                                let cap = wait_for_capture(&mut cam, &mut checker, false, |_| {})?;
                                Ok((cap.bytes, cap.width, cap.height))
                            })();

                            let mut matched_face_name: Option<String> = None;
                            let text = match result {
                                Ok((bytes, width, height)) => {
                                    match proxy.match_faces(&username, &bytes, width, height).await
                                    {
                                        Ok(faces) => {
                                            if let Some((name, _, _, _, _)) =
                                                faces.iter().find(|(_, _, _, passed, _)| *passed)
                                            {
                                                matched_face_name = Some(name.clone());
                                                "✓ Authentication successful".to_string()
                                            } else {
                                                "✗ Authentication failed".to_string()
                                            }
                                        }
                                        Err(e) => format!(
                                            "✗ DBus error: {}",
                                            dbus_error_message(&e)
                                        ),
                                    }
                                }
                                Err(e) => format!("✗ {}", e),
                            };

                            if let Some(face_name) = matched_face_name.as_ref() {
                                let mut child = face_list.first_child();
                                while let Some(widget) = child {
                                    if let Ok(row) = widget.clone().downcast::<libadwaita::ActionRow>() {
                                        row.remove_css_class("auth-match-highlight");
                                        if row.title().as_str() == face_name {
                                            row.add_css_class("auth-match-highlight");
                                            let row_clone = row.clone();
                                            glib::timeout_add_seconds_local_once(
                                                2,
                                                move || {
                                                    row_clone.remove_css_class("auth-match-highlight");
                                                },
                                            );
                                        }
                                    }
                                    child = widget.next_sibling();
                                }
                            }

                            let toast = libadwaita::Toast::new(&text);
                            if let Some(overlay) = window
                                .content()
                                .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                            {
                                overlay.add_toast(toast.clone());
                            }
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
                proxy,
                move |_| {
                    capture_dialog::show_capture_dialog(
                        &window,
                        &username,
                        None,
                        &proxy,
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
            ));

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
        }
    ));
}
