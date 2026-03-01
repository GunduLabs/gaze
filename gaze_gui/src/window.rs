use crate::capture_dialog;
use gaze_core::camera::Camera;
use gaze_core::config::Config;
use gaze_core::dbus::AuthProxy;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;

use std::cell::RefCell;
use std::rc::Rc;
use zbus::Connection;

type RefreshCb = Rc<dyn Fn()>;

pub fn build_window(app: &libadwaita::Application, username: &str) {
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
                            let faces = proxy.list_faces(&username).await.unwrap_or_default();

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

                                    let refine_btn =
                                        gtk4::Button::from_icon_name("view-refresh-symbolic");
                                    refine_btn.add_css_class("flat");
                                    let delete_btn =
                                        gtk4::Button::from_icon_name("user-trash-symbolic");
                                    delete_btn.add_css_class("flat");

                                    btn_box.append(&refine_btn);
                                    btn_box.append(&delete_btn);
                                    row.add_suffix(&btn_box);

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
                                                #[strong]
                                                username,
                                                #[strong]
                                                face_name,
                                                #[strong]
                                                refresh,
                                                #[strong]
                                                proxy,
                                                async move {
                                                    let _ = proxy
                                                        .remove_face(&username, &face_name)
                                                        .await;
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

            test_btn.connect_clicked(glib::clone!(
                #[weak]
                window,
                #[strong]
                username,
                #[weak]
                face_list,
                #[strong]
                proxy,
                move |btn| {
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
                        async move {
                            let config = Config::load().unwrap_or_default();
                            let t0 = std::time::Instant::now();
                            let result = std::thread::spawn(
                                move || -> anyhow::Result<(Vec<u8>, u32, u32)> {
                                    let mut cam = Camera::open(&config.cameras.rgb)?;
                                    let frame = cam.capture_frame()?;
                                    let cap = gaze_core::capture::frame_to_bytes(&frame)?;
                                    Ok((cap.bytes, cap.width, cap.height))
                                },
                            )
                            .join();

                            let (text, face_scores) = match result {
                                Ok(Ok((bytes, width, height))) => {
                                    match proxy.match_faces(&username, &bytes, width, height).await
                                    {
                                        Ok(faces) => {
                                            let scores: Vec<(String, f64)> = faces
                                                .iter()
                                                .map(|(name, _, pct, _, _)| (name.clone(), *pct))
                                                .collect();
                                            if faces.iter().any(|(_, _, _, passed, _)| *passed) {
                                                (
                                                    format!(
                                                        "✓ Authenticated ({}ms)",
                                                        t0.elapsed().as_millis()
                                                    ),
                                                    scores,
                                                )
                                            } else {
                                                (
                                                    format!(
                                                        "✗ Authentication failed ({}ms)",
                                                        t0.elapsed().as_millis()
                                                    ),
                                                    scores,
                                                )
                                            }
                                        }
                                        Err(e) => (format!("✗ DBus error: {}", e), Vec::new()),
                                    }
                                }
                                _ => ("✗ Capture failed".to_string(), Vec::new()),
                            };

                            if !face_scores.is_empty() {
                                let mut badges: Vec<gtk4::Label> = Vec::new();
                                let mut child = face_list.first_child();
                                while let Some(widget) = child {
                                    if let Some(row) =
                                        widget.downcast_ref::<libadwaita::ActionRow>()
                                    {
                                        let name = row.title().to_string();

                                        let mut prefix_child = row.first_child();
                                        while let Some(pc) = prefix_child {
                                            prefix_child = pc.next_sibling();
                                            if pc.widget_name() == "match-badge" {
                                                pc.unparent();
                                            }
                                        }

                                        if let Some((_, pct)) =
                                            face_scores.iter().find(|(n, _)| n == &name)
                                        {
                                            let badge =
                                                gtk4::Label::new(Some(&format!("{:.1}%", pct)));
                                            badge.set_widget_name("match-badge");
                                            badge.add_css_class("heading");
                                            if *pct >= 50.0 {
                                                badge.add_css_class("success");
                                            } else {
                                                badge.add_css_class("error");
                                            }
                                            badge.set_margin_end(8);
                                            badge.set_valign(gtk4::Align::Center);
                                            row.add_prefix(&badge);
                                            badges.push(badge);
                                        }
                                    }
                                    child = widget.next_sibling();
                                }

                                glib::timeout_add_seconds_local_once(3, move || {
                                    for badge in badges {
                                        badge.unparent();
                                    }
                                });
                            }

                            let toast = libadwaita::Toast::new(&text);
                            if let Some(overlay) = window
                                .content()
                                .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                            {
                                overlay.add_toast(toast);
                            }
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
