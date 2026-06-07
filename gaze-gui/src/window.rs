use crate::capture_dialog;
use gaze_core::config::{Config, DEFAULT_RGB_CAMERA, SecurityLevel};
use gaze_core::dbus::{
    GazeProxy, apply_config_to_daemon, dbus_error_message, dbus_is_file_not_found,
    load_config_from_daemon,
};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;

use enumflags2::BitFlag;
use futures::StreamExt;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::OnceLock;
use zbus::Connection;
use zbus_polkit::policykit1::{AuthorityProxy, CheckAuthorizationFlags, Subject};

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

    let window = libadwaita::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Configuration")
        .default_width(600)
        .default_height(700)
        .build();

    let toolbar_view = libadwaita::ToolbarView::new();
    let header_bar = libadwaita::HeaderBar::new();
    toolbar_view.add_top_bar(&header_bar);

    let banner = libadwaita::Banner::new("Settings are locked");
    banner.set_button_label(Some("Unlock…"));
    toolbar_view.add_top_bar(&banner);

    let scrolled = gtk4::ScrolledWindow::builder()
        .hscrollbar_policy(gtk4::PolicyType::Never)
        .build();

    let page = libadwaita::PreferencesPage::new();
    scrolled.set_child(Some(&page));
    toolbar_view.set_content(Some(&scrolled));
    window.set_content(Some(&toolbar_view));

    scrolled.set_sensitive(false);
    banner.set_revealed(true);

    let security_group = libadwaita::PreferencesGroup::new();
    security_group.set_title("Security");
    page.add(&security_group);

    let level_row = libadwaita::ComboRow::new();
    level_row.set_title("Security Level");
    level_row.set_subtitle("Adjust the balance between speed and security");
    let level_model = gtk4::StringList::new(&["Low", "Medium", "High", "Maximum", "Custom"]);
    level_row.set_model(Some(&level_model));
    security_group.add(&level_row);

    let detector_row = libadwaita::EntryRow::new();
    detector_row.set_title("Detector Model");
    security_group.add(&detector_row);

    let recognizer_row = libadwaita::EntryRow::new();
    recognizer_row.set_title("Recognizer Model");
    security_group.add(&recognizer_row);

    let threshold_row = libadwaita::SpinRow::with_range(0.0, 1.0, 0.01);
    threshold_row.set_digits(3);
    threshold_row.set_title("Recognizer Threshold");
    threshold_row.set_subtitle("Minimum similarity for a match");
    security_group.add(&threshold_row);

    let hardware_group = libadwaita::PreferencesGroup::new();
    hardware_group.set_title("Hardware");
    page.add(&hardware_group);

    let cameras = gaze_core::camera::enumerate_cameras()
        .unwrap_or_else(|_| vec![("Primary Camera".to_string(), DEFAULT_RGB_CAMERA.to_string())]);
    let cam_names = cameras.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();

    let camera_row = libadwaita::ComboRow::new();
    camera_row.set_title("RGB Camera Source");
    let cam_model =
        gtk4::StringList::new(&cam_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    camera_row.set_model(Some(&cam_model));
    hardware_group.add(&camera_row);

    let ir_row = libadwaita::EntryRow::new();
    ir_row.set_title("IR Camera Device");
    ir_row.set_tooltip_text(Some(
        "Infrared camera node, e.g. /dev/video2 (blank for none). Run `gaze discover`.",
    ));
    hardware_group.add(&ir_row);

    let emitter_row = libadwaita::ActionRow::new();
    emitter_row.set_title("Drive IR Emitter");
    emitter_row.set_subtitle("Turn the camera's IR LED on during authentication");
    let emitter_switch = gtk4::Switch::new();
    emitter_switch.set_valign(gtk4::Align::Center);
    emitter_row.add_suffix(&emitter_switch);
    hardware_group.add(&emitter_row);

    let dark_luma_threshold_row = libadwaita::SpinRow::with_range(0.0, 255.0, 1.0);
    dark_luma_threshold_row.set_digits(0);
    dark_luma_threshold_row.set_title("Darkness Cutoff");
    dark_luma_threshold_row.set_subtitle("Reject frames below this mean brightness (0-255)");
    hardware_group.add(&dark_luma_threshold_row);

    let enrollment_group = libadwaita::PreferencesGroup::new();
    enrollment_group.set_title("Enrollment");
    page.add(&enrollment_group);

    let templates_row = libadwaita::SpinRow::with_range(1.0, 50.0, 1.0);
    templates_row.set_title("Max Templates");
    templates_row.set_subtitle("Number of capture sets stored per face");
    enrollment_group.add(&templates_row);

    let liveness_group = libadwaita::PreferencesGroup::new();
    liveness_group.set_title("Liveness Anti-Spoofing");
    page.add(&liveness_group);

    let liveness_enabled_row = libadwaita::ActionRow::new();
    liveness_enabled_row.set_title("Enable Liveness Spoof Prevention");
    liveness_enabled_row.set_subtitle("Analyze face depth/reflectance to prevent photo spoofing");
    let liveness_enabled_switch = gtk4::Switch::new();
    liveness_enabled_switch.set_valign(gtk4::Align::Center);
    liveness_enabled_row.add_suffix(&liveness_enabled_switch);
    liveness_group.add(&liveness_enabled_row);

    let liveness_threshold_row = libadwaita::SpinRow::with_range(0.0, 1.0, 0.01);
    liveness_threshold_row.set_digits(3);
    liveness_threshold_row.set_title("Liveness Threshold");
    liveness_threshold_row.set_subtitle("Minimum spoof prevention confidence");
    liveness_group.add(&liveness_threshold_row);

    let liveness_max_frames_row = libadwaita::SpinRow::with_range(1.0, 500.0, 1.0);
    liveness_max_frames_row.set_digits(0);
    liveness_max_frames_row.set_title("Liveness Max Frames");
    liveness_max_frames_row.set_subtitle("Maximum frames analyzed for liveness verification");
    liveness_group.add(&liveness_max_frames_row);

    let auth_group = libadwaita::PreferencesGroup::new();
    auth_group.set_title("Auth");
    page.add(&auth_group);

    let abort_ssh_row = libadwaita::ActionRow::new();
    abort_ssh_row.set_title("Abort if SSH");
    abort_ssh_row.set_subtitle("Prevent authentication over SSH connections");
    let abort_ssh_switch = gtk4::Switch::new();
    abort_ssh_switch.set_valign(gtk4::Align::Center);
    abort_ssh_row.add_suffix(&abort_ssh_switch);
    auth_group.add(&abort_ssh_row);

    let abort_lid_row = libadwaita::ActionRow::new();
    abort_lid_row.set_title("Abort if Lid Closed");
    abort_lid_row.set_subtitle("Prevent authentication when the laptop lid is closed");
    let abort_lid_switch = gtk4::Switch::new();
    abort_lid_switch.set_valign(gtk4::Align::Center);
    abort_lid_row.add_suffix(&abort_lid_switch);
    auth_group.add(&abort_lid_row);

    let require_confirm_row = libadwaita::ActionRow::new();
    require_confirm_row.set_title("Require Confirmation");
    require_confirm_row
        .set_subtitle("Require pressing Enter or clicking OK to authorize after face matches");
    let require_confirm_switch = gtk4::Switch::new();
    require_confirm_switch.set_valign(gtk4::Align::Center);
    require_confirm_row.add_suffix(&require_confirm_switch);
    auth_group.add(&require_confirm_row);

    let update_custom_visibility =
        move |row: &libadwaita::ComboRow,
              det: &libadwaita::EntryRow,
              rec: &libadwaita::EntryRow,
              thr: &libadwaita::SpinRow| {
            let is_custom = row.selected() == 4;
            det.set_visible(is_custom);
            rec.set_visible(is_custom);
            thr.set_visible(is_custom);
        };

    let update_liveness_visibility =
        move |sw: &gtk4::Switch, thr: &libadwaita::SpinRow, fr: &libadwaita::SpinRow| {
            let active = sw.is_active();
            thr.set_visible(active);
            fr.set_visible(active);
        };

    liveness_enabled_switch.connect_active_notify(glib::clone!(
        #[weak]
        liveness_threshold_row,
        #[weak]
        liveness_max_frames_row,
        move |sw| {
            update_liveness_visibility(sw, &liveness_threshold_row, &liveness_max_frames_row);
        }
    ));

    let is_loading = Rc::new(std::cell::Cell::new(true));

    level_row.connect_selected_notify(glib::clone!(
        #[weak]
        detector_row,
        #[weak]
        recognizer_row,
        #[weak]
        threshold_row,
        move |row| {
            update_custom_visibility(row, &detector_row, &recognizer_row, &threshold_row);
        }
    ));

    let apply_changes = glib::clone!(
        #[weak]
        overlay,
        #[weak]
        level_row,
        #[weak]
        detector_row,
        #[weak]
        recognizer_row,
        #[weak]
        threshold_row,
        #[weak]
        camera_row,
        #[weak]
        ir_row,
        #[weak]
        emitter_switch,
        #[weak]
        dark_luma_threshold_row,
        #[weak]
        templates_row,
        #[weak]
        liveness_enabled_switch,
        #[weak]
        liveness_threshold_row,
        #[weak]
        liveness_max_frames_row,
        #[weak]
        require_confirm_switch,
        #[weak]
        abort_ssh_switch,
        #[weak]
        abort_lid_switch,
        #[strong]
        cameras,
        #[strong]
        config,
        #[strong]
        is_loading,
        move || {
            if is_loading.get() {
                return;
            }

            let mut cfg = config.borrow_mut();
            match level_row.selected() {
                0 => cfg.security = SecurityLevel::low(),
                1 => cfg.security = SecurityLevel::medium(),
                2 => cfg.security = SecurityLevel::high(),
                3 => cfg.security = SecurityLevel::maximum(),
                4 => {
                    cfg.security = SecurityLevel::custom(
                        detector_row.text().to_string(),
                        recognizer_row.text().to_string(),
                        threshold_row.value(),
                    );
                }
                _ => {}
            }

            let cam_idx = camera_row.selected() as usize;
            if let Some((_, target)) = cameras.get(cam_idx) {
                cfg.cameras.rgb = target.clone();
            }
            cfg.cameras.ir = ir_row.text().trim().to_string();
            cfg.cameras.emitter_enabled = emitter_switch.is_active();
            cfg.cameras.dark_luma_threshold = dark_luma_threshold_row.value() as u8;
            cfg.enrollment.max_templates = templates_row.value() as u32;
            cfg.liveness.enabled = liveness_enabled_switch.is_active();
            cfg.liveness.threshold = liveness_threshold_row.value();
            cfg.liveness.max_frames = liveness_max_frames_row.value() as u32;
            cfg.auth.require_confirmation = require_confirm_switch.is_active();
            cfg.auth.abort_if_ssh = abort_ssh_switch.is_active();
            cfg.auth.abort_if_lid_closed = abort_lid_switch.is_active();

            let cfg_to_apply = cfg.clone();
            drop(cfg);

            glib::MainContext::default().spawn_local(glib::clone!(
                #[weak]
                overlay,
                #[strong]
                cfg_to_apply,
                async move {
                    let result = async {
                        let conn = Connection::system().await?;
                        let proxy = GazeProxy::new(&conn).await?;
                        apply_config_to_daemon(&proxy, &cfg_to_apply).await
                    }
                    .await;

                    if let Err(e) = result {
                        overlay.add_toast(libadwaita::Toast::new(&format!(
                            "Failed to apply config: {}",
                            e
                        )));
                    }
                }
            ));
        }
    );

    level_row.connect_selected_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    camera_row.connect_selected_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    threshold_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    templates_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    detector_row.connect_apply(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    recognizer_row.connect_apply(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));

    require_confirm_switch.connect_active_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    abort_ssh_switch.connect_active_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    abort_lid_switch.connect_active_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));

    dark_luma_threshold_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    ir_row.connect_apply(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    emitter_switch.connect_active_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    liveness_enabled_switch.connect_active_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    liveness_threshold_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    liveness_max_frames_row.connect_value_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));

    {
        let cfg = config.borrow();
        let level_idx = match cfg.security.level.as_str() {
            "low" => 0,
            "medium" => 1,
            "high" => 2,
            "maximum" => 3,
            "custom" => 4,
            _ => 1,
        };
        level_row.set_selected(level_idx);
        update_custom_visibility(&level_row, &detector_row, &recognizer_row, &threshold_row);

        let (det, rec, thr) = if cfg.security.level == "custom" {
            (
                cfg.security.detector.clone(),
                cfg.security.recognizer.clone(),
                cfg.security.threshold,
            )
        } else {
            (
                cfg.security.detector().to_string(),
                cfg.security.recognizer().to_string(),
                cfg.security.threshold() as f64,
            )
        };
        detector_row.set_text(&det);
        recognizer_row.set_text(&rec);
        threshold_row.set_value(thr);

        let cam_idx = cameras
            .iter()
            .position(|(_, t)| t == &cfg.cameras.rgb)
            .unwrap_or(0);
        camera_row.set_selected(cam_idx as u32);
        ir_row.set_text(&cfg.cameras.ir);
        emitter_switch.set_active(cfg.cameras.emitter_enabled);
        dark_luma_threshold_row.set_value(cfg.cameras.dark_luma_threshold as f64);
        templates_row.set_value(cfg.enrollment.max_templates as f64);
        liveness_enabled_switch.set_active(cfg.liveness.enabled);
        liveness_threshold_row.set_value(cfg.liveness.threshold);
        liveness_max_frames_row.set_value(cfg.liveness.max_frames as f64);
        require_confirm_switch.set_active(cfg.auth.require_confirmation);
        abort_ssh_switch.set_active(cfg.auth.abort_if_ssh);
        abort_lid_switch.set_active(cfg.auth.abort_if_lid_closed);

        update_liveness_visibility(
            &liveness_enabled_switch,
            &liveness_threshold_row,
            &liveness_max_frames_row,
        );
    }
    is_loading.set(false);

    // Wire the Unlock button up front so it is always responsive the moment the
    // dialog is shown, independent of the background state task below. Connecting
    // it inside that task meant the button stayed dead until several DBus
    // round-trips finished, and a failing `receive_changed()` could panic the
    // task before the handler was ever attached. Each click opens the system bus
    // and runs an interactive polkit check; failures are logged rather than
    // silently swallowed so a non-working button is diagnosable.
    banner.connect_button_clicked(glib::clone!(
        #[weak]
        banner,
        #[weak]
        scrolled,
        move |_| {
            glib::MainContext::default().spawn_local(glib::clone!(
                #[weak]
                banner,
                #[weak]
                scrolled,
                async move {
                    let conn = match Connection::system().await {
                        Ok(conn) => conn,
                        Err(e) => {
                            eprintln!("gaze-gui: system bus connection failed: {e}");
                            return;
                        }
                    };
                    let authority = match AuthorityProxy::new(&conn).await {
                        Ok(authority) => authority,
                        Err(e) => {
                            eprintln!("gaze-gui: polkit proxy creation failed: {e}");
                            return;
                        }
                    };
                    let subject = match Subject::new_for_owner(std::process::id(), None, None) {
                        Ok(subject) => subject,
                        Err(e) => {
                            eprintln!("gaze-gui: polkit subject creation failed: {e}");
                            return;
                        }
                    };

                    match authority
                        .check_authorization(
                            &subject,
                            "com.gundulabs.gaze.manage-config",
                            &HashMap::new(),
                            CheckAuthorizationFlags::AllowUserInteraction.into(),
                            "",
                        )
                        .await
                    {
                        Ok(res) => {
                            banner.set_revealed(!res.is_authorized);
                            scrolled.set_sensitive(res.is_authorized);
                        }
                        Err(e) => eprintln!("gaze-gui: polkit CheckAuthorization failed: {e}"),
                    }
                }
            ));
        }
    ));

    // Reflect the current authorization state on open and keep it in sync when
    // polkit's authorizations change underneath us.
    glib::MainContext::default().spawn_local(glib::clone!(
        #[weak]
        banner,
        #[weak]
        scrolled,
        async move {
            let Ok(conn) = Connection::system().await else {
                return;
            };
            let Ok(authority) = AuthorityProxy::new(&conn).await else {
                return;
            };

            let check_auth = |auth: AuthorityProxy<'static>| async move {
                let subject = Subject::new_for_owner(std::process::id(), None, None).ok()?;

                auth.check_authorization(
                    &subject,
                    "com.gundulabs.gaze.manage-config",
                    &HashMap::new(),
                    CheckAuthorizationFlags::empty(),
                    "",
                )
                .await
                .ok()
                .map(|res| res.is_authorized)
            };

            let update_ui = glib::clone!(
                #[weak]
                banner,
                #[weak]
                scrolled,
                move |allowed: bool| {
                    banner.set_revealed(!allowed);
                    scrolled.set_sensitive(allowed);
                }
            );

            if let Some(allowed) = check_auth(authority.clone()).await {
                update_ui(allowed);
            }

            let Ok(mut changed_stream) = authority.receive_changed().await else {
                return;
            };

            while changed_stream.next().await.is_some() {
                if let Some(allowed) = check_auth(authority.clone()).await {
                    update_ui(allowed);
                }
            }
        }
    ));

    glib::MainContext::default().spawn_local(glib::clone!(
        #[weak]
        level_row,
        #[weak]
        detector_row,
        #[weak]
        recognizer_row,
        #[weak]
        threshold_row,
        #[weak]
        camera_row,
        #[weak]
        ir_row,
        #[weak]
        emitter_switch,
        #[weak]
        dark_luma_threshold_row,
        #[weak]
        templates_row,
        #[weak]
        liveness_enabled_switch,
        #[weak]
        liveness_threshold_row,
        #[weak]
        liveness_max_frames_row,
        #[weak]
        require_confirm_switch,
        #[weak]
        abort_ssh_switch,
        #[weak]
        abort_lid_switch,
        #[strong]
        cameras,
        #[strong]
        config,
        #[strong]
        is_loading,
        async move {
            let load_result = async {
                let conn = Connection::system().await?;
                let proxy = GazeProxy::new(&conn).await?;
                load_config_from_daemon(&proxy).await
            }
            .await;

            if let Ok(cfg) = load_result {
                is_loading.set(true);
                let level_idx = match cfg.security.level.as_str() {
                    "low" => 0,
                    "medium" => 1,
                    "high" => 2,
                    "maximum" => 3,
                    "custom" => 4,
                    _ => 1,
                };
                level_row.set_selected(level_idx);

                let (det, rec, thr) = if cfg.security.level == "custom" {
                    (
                        cfg.security.detector.clone(),
                        cfg.security.recognizer.clone(),
                        cfg.security.threshold,
                    )
                } else {
                    (
                        cfg.security.detector().to_string(),
                        cfg.security.recognizer().to_string(),
                        cfg.security.threshold() as f64,
                    )
                };
                detector_row.set_text(&det);
                recognizer_row.set_text(&rec);
                threshold_row.set_value(thr);

                let cam_idx = cameras
                    .iter()
                    .position(|(_, t)| t == &cfg.cameras.rgb)
                    .unwrap_or(0);
                camera_row.set_selected(cam_idx as u32);
                ir_row.set_text(&cfg.cameras.ir);
                emitter_switch.set_active(cfg.cameras.emitter_enabled);
                dark_luma_threshold_row.set_value(cfg.cameras.dark_luma_threshold as f64);
                templates_row.set_value(cfg.enrollment.max_templates as f64);
                liveness_enabled_switch.set_active(cfg.liveness.enabled);
                liveness_threshold_row.set_value(cfg.liveness.threshold);
                liveness_max_frames_row.set_value(cfg.liveness.max_frames as f64);
                require_confirm_switch.set_active(cfg.auth.require_confirmation);
                abort_ssh_switch.set_active(cfg.auth.abort_if_ssh);
                abort_lid_switch.set_active(cfg.auth.abort_if_lid_closed);

                update_liveness_visibility(
                    &liveness_enabled_switch,
                    &liveness_threshold_row,
                    &liveness_max_frames_row,
                );

                *config.borrow_mut() = cfg;
                is_loading.set(false);
            }
        }
    ));

    window.present();
}

fn show_daemon_pending_toast(window: &libadwaita::ApplicationWindow) {
    if let Some(overlay) = window
        .content()
        .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
    {
        overlay.add_toast(libadwaita::Toast::new("Connecting to the Gaze daemon…"));
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

    // Shared by the synchronously-wired toolbar buttons and populated once the
    // daemon connection lands in the task below.
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
                        if let Some(overlay) = window
                            .content()
                            .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                        {
                            overlay.add_toast(libadwaita::Toast::new("Failed to claim device"));
                        }
                        btn.set_sensitive(true);
                        return;
                    }

                    if proxy.verify_start("any").await.is_err() {
                        if let Some(overlay) = window
                            .content()
                            .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                        {
                            overlay.add_toast(libadwaita::Toast::new(
                                "Daemon error starting verification",
                            ));
                        }
                        let _ = proxy.release().await;
                        btn.set_sensitive(true);
                        return;
                    }

                    let mut text = "✗ Verification failed".to_string();
                    let mut matched_face: Option<String> = None;

                    if let Ok(mut stream) = proxy.receive_verify_status().await {
                        while let Some(signal) = stream.next().await {
                            if let Ok(args) = signal.args() {
                                let res = *args.result();
                                if res == gaze_core::dbus::VerifyResult::VerifyMatch {
                                    text = "✓ Authentication successful".to_string();
                                    let faces = args.faces();
                                    matched_face = faces
                                        .iter()
                                        .find(|(_, _, _, p, _)| *p)
                                        .map(|(n, _, _, _, _)| n.clone());
                                    break;
                                } else {
                                    text = "✗ Authentication failed".to_string();
                                    break;
                                }
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
                    if let Err(err) = proxy.claim(&username).await {
                        let toast = libadwaita::Toast::new(&format!(
                            "Failed to claim device: {}",
                            dbus_error_message(&err)
                        ));
                        if let Some(overlay) = window
                            .content()
                            .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                        {
                            overlay.add_toast(toast);
                        }
                        return;
                    }

                    let camera_device = match load_config_from_daemon(&proxy).await {
                        Ok(cfg) => gaze_core::camera::resolve_source(&cfg.cameras).0,
                        Err(_) => "primary".to_string(),
                    };

                    capture_dialog::show_capture_dialog(
                        &window,
                        &username,
                        None,
                        &proxy,
                        &camera_device,
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
            let Ok(conn) = Connection::system().await else {
                tracing::error!("Failed to connect to system DBus");
                status_page.set_description(Some("Failed to connect to system DBus"));
                return;
            };

            let Ok(proxy) = GazeProxy::new(&conn).await else {
                tracing::error!("Failed to create GazeProxy");
                status_page.set_description(Some("Failed to create GazeProxy"));
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
                                    Rc::new(faces.iter().map(|(name, _): &(String, u32)| name.clone()).collect());

                                for (face_name, count) in faces {
                                    let row = libadwaita::ActionRow::new();
                                    row.set_title(&face_name);
                                    row.set_subtitle(&format!(
                                        "{} template{}",
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
                                                    if let Err(err) = proxy.claim(&username).await {
                                                        let toast = libadwaita::Toast::new(&format!(
                                                            "Failed to claim device: {}",
                                                            dbus_error_message(&err)
                                                        ));
                                                        if let Some(overlay) = window
                                                            .content()
                                                            .and_then(|c| c.downcast::<libadwaita::ToastOverlay>().ok())
                                                        {
                                                            overlay.add_toast(toast);
                                                        }
                                                        return;
                                                    }

                                                     let camera_device = match load_config_from_daemon(&proxy).await {
                                                         Ok(cfg) => gaze_core::camera::resolve_source(&cfg.cameras).0,
                                                         Err(_) => "primary".to_string(),
                                                     };

                                                     capture_dialog::show_capture_dialog(
                                                        &window,
                                                        &username,
                                                        Some(&face_name),
                                                        &proxy,
                                                        &camera_device,
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
                                                        .delete_face(&username, &face_name)
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

        }
    ));
}
