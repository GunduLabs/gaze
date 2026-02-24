use crate::enroll_dialog;
use gaze_common::camera::Camera;
use gaze_common::config::Config;
use gaze_common::dbus::AuthProxy;
use gaze_common::face::FaceChecker;
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;

use std::cell::RefCell;
use std::rc::Rc;
use zbus::Connection;

type RefreshCb = Rc<dyn Fn()>;

pub fn build_window(app: &libadwaita::Application, username_str: &str) {
    let username = Rc::new(username_str.to_string());
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
    status_page.set_description(Some("Press + to add your first face"));
    status_page.set_visible(false);
    content.append(&status_page);

    clamp.set_child(Some(&content));
    scroll.set_child(Some(&clamp));
    main_box.append(&scroll);

    let refresh: Rc<RefCell<Option<RefreshCb>>> = Rc::new(RefCell::new(None));

    {
        let fl = face_list.clone();
        let sp = status_page.clone();
        let un = username.clone();
        let ww = glib::SendWeakRef::from(window.downgrade());
        let refresh_ptr = refresh.clone();

        *refresh.borrow_mut() = Some(Rc::new(move || {
            let fl = fl.clone();
            let sp = sp.clone();
            let un = un.clone();
            let ww = ww.clone();
            let refresh_ptr = refresh_ptr.clone();

            glib::MainContext::default().spawn_local(async move {
                let Ok(conn) = Connection::system().await else {
                    return;
                };
                let Ok(proxy) = AuthProxy::new(&conn).await else {
                    return;
                };
                let faces = proxy.list_faces(&un).await.unwrap_or_default();

                while let Some(child) = fl.first_child() {
                    fl.remove(&child);
                }

                if faces.is_empty() {
                    sp.set_visible(true);
                    fl.set_visible(false);
                } else {
                    sp.set_visible(false);
                    fl.set_visible(true);

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

                        let refine_btn = gtk4::Button::from_icon_name("view-refresh-symbolic");
                        refine_btn.add_css_class("flat");
                        let delete_btn = gtk4::Button::from_icon_name("user-trash-symbolic");
                        delete_btn.add_css_class("flat");

                        btn_box.append(&refine_btn);
                        btn_box.append(&delete_btn);
                        row.add_suffix(&btn_box);

                        {
                            let un = un.clone();
                            let fn_ = face_name.clone();
                            let ww = ww.clone();
                            let refresh_ptr = refresh_ptr.clone();
                            refine_btn.connect_clicked(move |_| {
                                if let Some(win) = ww.upgrade() {
                                    let r = refresh_ptr.clone();
                                    enroll_dialog::show_enroll_dialog(
                                        &win,
                                        &un,
                                        Some(&fn_),
                                        move || {
                                            if let Some(f) = r.borrow().as_ref() {
                                                f();
                                            }
                                        },
                                    );
                                }
                            });
                        }

                        {
                            let un = un.clone();
                            let fn_ = face_name.clone();
                            let refresh_ptr = refresh_ptr.clone();
                            delete_btn.connect_clicked(move |_| {
                                let un = un.clone();
                                let fn_ = fn_.clone();
                                let refresh_ptr = refresh_ptr.clone();
                                glib::MainContext::default().spawn_local(async move {
                                    if let Ok(conn) = Connection::system().await
                                        && let Ok(proxy) = AuthProxy::new(&conn).await
                                    {
                                        let _ = proxy.remove_face(&un, &fn_).await;
                                        if let Some(f) = refresh_ptr.borrow().as_ref() {
                                            f();
                                        }
                                    }
                                });
                            });
                        }

                        fl.append(&row);
                    }
                }
            });
        }));
    }

    if let Some(f) = refresh.borrow().as_ref() {
        f();
    }

    {
        let un = username.clone();
        let ww = glib::SendWeakRef::from(window.downgrade());
        test_btn.connect_clicked(move |btn| {
            btn.set_sensitive(false);
            let un = un.clone();
            let b = btn.clone();
            let ww = ww.clone();
            glib::MainContext::default().spawn_local(async move {
                let config = Config::load().unwrap_or_default();
                let result = std::thread::spawn(move || {
                    let mut cam = Camera::open(&config.cameras.rgb)?;
                    let mut checker = FaceChecker::new()?;
                    gaze_common::capture::wait_for_capture(&mut cam, &mut checker, false, |_| {})
                })
                .join();

                let toast_text = match result {
                    Ok(Ok(cap)) => match Connection::system().await {
                        Ok(conn) => match AuthProxy::new(&conn).await {
                            Ok(proxy) => {
                                match proxy
                                    .authenticate(&un, &cap.bytes, cap.width, cap.height)
                                    .await
                                {
                                    Ok(face) if !face.is_empty() => {
                                        format!("✓ Authenticated as: {}", face)
                                    }
                                    Ok(_) => "✗ Authentication failed".to_string(),
                                    Err(e) => format!("✗ DBus error: {}", e),
                                }
                            }
                            _ => "Proxy error".to_string(),
                        },
                        _ => "DBus error".to_string(),
                    },
                    _ => "Capture failed".to_string(),
                };

                if let Some(win) = ww.upgrade() {
                    let toast = libadwaita::Toast::new(&toast_text);
                    if let Some(overlay) = win
                        .content()
                        .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                    {
                        overlay.add_toast(toast);
                    }
                }
                b.set_sensitive(true);
            });
        });
    }

    {
        let un = username.to_string();
        let refresh_ptr = refresh.clone();
        let ww = glib::SendWeakRef::from(window.downgrade());
        add_btn.connect_clicked(move |_| {
            if let Some(win) = ww.upgrade() {
                let r = refresh_ptr.clone();
                enroll_dialog::show_enroll_dialog(&win, &un, None, move || {
                    if let Some(f) = r.borrow().as_ref() {
                        f();
                    }
                });
            }
        });
    }

    let toast_overlay = libadwaita::ToastOverlay::new();
    toast_overlay.set_child(Some(&main_box));
    window.set_content(Some(&toast_overlay));
    window.present();
}
