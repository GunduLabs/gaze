use crate::capture_dialog;
use gaze_core::capture::{init_camera_and_checker, wait_for_capture};
use gaze_core::config::Config;
use gaze_core::dbus::{AuthProxy, dbus_error_message, dbus_is_file_not_found};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;
use zbus::Connection;

type RefreshCb = Rc<dyn Fn()>;

fn format_dbus_error(err: &zbus::Error) -> String {
    dbus_error_message(err)
}

fn is_file_not_found_error(err: &zbus::Error) -> bool {
    dbus_is_file_not_found(err)
}

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

    header.pack_end(&add_btn);
    header.pack_end(&test_btn);

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
                                    if is_file_not_found_error(&err) {
                                        Vec::new()
                                    } else {
                                        let toast = libadwaita::Toast::new(&format!(
                                            "Failed to load faces: {}",
                                            format_dbus_error(&err)
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
                                                                    format_dbus_error(&err)
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
                                                            format_dbus_error(&err)
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
                                let config = Config::load().unwrap_or_default();
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
                                            format_dbus_error(&e)
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
        }
    ));
}
