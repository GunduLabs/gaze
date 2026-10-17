// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gaze_core::config::{
    AuthConfig, CameraConfig, Config, DEFAULT_RGB_CAMERA, HYBRID_POLICY_LABELS,
    INFERENCE_DEVICE_LABELS, INFERENCE_EXECUTION_PROVIDER_LABELS, InferenceConfig,
    MAX_ENROLLMENT_FACE_SIZE_RATIO, MAX_IR_FRAME_DIMENSION, MAX_LIVENESS_MAX_SECONDS,
    MIN_ENROLLMENT_FACE_SIZE_RATIO, MIN_LIVENESS_MAX_SECONDS, MODEL_QUALITY_LABELS,
    PARALLEL_CAPTURE_LABELS, SECURITY_LEVEL_LABELS, START_DELAY_SCOPE_LABELS, SecurityLevel,
};
use gaze_core::dbus::{
    GazeProxy, apply_config_to_daemon, apply_config_with_keyring_to_daemon, connect_gaze,
    load_config_with_keyring_from_daemon,
};
use gaze_vision::camera::{is_listed_source, source_index};
use gtk4::glib;
use gtk4::prelude::*;
use libadwaita::prelude::*;

use futures::StreamExt;
use gaze_core::polkit::{ALLOW_USER_INTERACTION, AuthorityProxy, NO_INTERACTION, Subject};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;
use zbus::Connection;

const CONFIG_APPLY_DEBOUNCE: Duration = Duration::from_millis(400);

fn custom_rows_visible(selected: u32) -> bool {
    selected == SecurityLevel::CUSTOM_LEVEL_INDEX
}

fn set_custom_config_rows_visible(
    level_row: &libadwaita::ComboRow,
    detector_row: &libadwaita::ComboRow,
    recognizer_row: &libadwaita::ComboRow,
    rgb_threshold_row: &libadwaita::SpinRow,
    ir_threshold_row: &libadwaita::SpinRow,
    hybrid_row: &libadwaita::ComboRow,
) {
    let is_custom = custom_rows_visible(level_row.selected());
    detector_row.set_visible(is_custom);
    recognizer_row.set_visible(is_custom);
    rgb_threshold_row.set_visible(is_custom);
    ir_threshold_row.set_visible(is_custom);
    hybrid_row.set_visible(is_custom);
}

fn start_delay_scope_visible(start_delay_value: f64) -> bool {
    start_delay_value > 0.0
}

fn set_start_delay_scope_row_visible(
    start_delay_row: &libadwaita::SpinRow,
    scope_row: &libadwaita::ComboRow,
) {
    scope_row.set_visible(start_delay_scope_visible(start_delay_row.value()));
}

fn liveness_rows_visible(enabled: bool) -> bool {
    enabled
}

fn set_liveness_config_rows_visible(
    enabled_switch: &gtk4::Switch,
    threshold_row: &libadwaita::SpinRow,
    max_seconds_row: &libadwaita::SpinRow,
) {
    let active = liveness_rows_visible(enabled_switch.is_active());
    threshold_row.set_visible(active);
    max_seconds_row.set_visible(active);
}

fn keyring_sensitivity(
    keyring_active: bool,
    kwallet_active: bool,
    liveness_active: bool,
    encrypt_active: bool,
) -> (bool, bool, bool, bool) {
    let keyring_enabled = keyring_active || kwallet_active;
    let prerequisites = liveness_active && encrypt_active;
    (
        keyring_active || prerequisites,
        kwallet_active || prerequisites,
        !keyring_enabled,
        !keyring_enabled,
    )
}

fn set_keyring_config_sensitivity(
    keyring_switch: &gtk4::Switch,
    kwallet_switch: &gtk4::Switch,
    liveness_switch: &gtk4::Switch,
    encrypt_templates_switch: &gtk4::Switch,
) {
    let (keyring_sensitive, kwallet_sensitive, liveness_sensitive, encrypt_sensitive) =
        keyring_sensitivity(
            keyring_switch.is_active(),
            kwallet_switch.is_active(),
            liveness_switch.is_active(),
            encrypt_templates_switch.is_active(),
        );
    keyring_switch.set_sensitive(keyring_sensitive);
    kwallet_switch.set_sensitive(kwallet_sensitive);
    liveness_switch.set_sensitive(liveness_sensitive);
    encrypt_templates_switch.set_sensitive(encrypt_sensitive);
}

fn inference_device_visible(provider: &str) -> bool {
    provider == "openvino"
}

fn set_inference_device_row_visible(
    execution_provider_row: &libadwaita::ComboRow,
    device_row: &libadwaita::ComboRow,
) {
    let provider =
        InferenceConfig::execution_provider_from_index(execution_provider_row.selected() as usize);
    device_row.set_visible(inference_device_visible(provider));
}

type SharedProxy = Rc<RefCell<Option<Rc<GazeProxy<'static>>>>>;

async fn shared_proxy(cell: &SharedProxy) -> zbus::Result<Rc<GazeProxy<'static>>> {
    if let Some(proxy) = cell.borrow().clone() {
        return Ok(proxy);
    }
    let proxy = Rc::new(connect_gaze().await?);
    *cell.borrow_mut() = Some(proxy.clone());
    Ok(proxy)
}

type ConfigWriteFuture = futures::future::LocalBoxFuture<'static, anyhow::Result<()>>;

type ConfigWriter = Rc<dyn Fn(Config) -> ConfigWriteFuture>;

type ConfigWriteErrorSink = Rc<dyn Fn(String)>;

struct ConfigApplyQueue {
    write: ConfigWriter,
    report_error: ConfigWriteErrorSink,
    pending: Option<Config>,
    in_flight: bool,
    debounce: Option<glib::SourceId>,
}

type ApplyQueue = Rc<RefCell<ConfigApplyQueue>>;

fn new_apply_queue(write: ConfigWriter, report_error: ConfigWriteErrorSink) -> ApplyQueue {
    Rc::new(RefCell::new(ConfigApplyQueue {
        write,
        report_error,
        pending: None,
        in_flight: false,
        debounce: None,
    }))
}

fn drain_config_apply_queue(queue: &ApplyQueue) {
    {
        let mut q = queue.borrow_mut();
        if q.in_flight || q.pending.is_none() {
            return;
        }
        q.in_flight = true;
    }

    glib::MainContext::default().spawn_local(glib::clone!(
        #[strong]
        queue,
        async move {
            loop {
                let next = queue.borrow_mut().pending.take();
                let Some(cfg) = next else {
                    break;
                };

                let write = queue.borrow().write.clone();
                if let Err(e) = write(cfg).await {
                    let report_error = queue.borrow().report_error.clone();
                    report_error(format!("Failed to apply config: {e}"));
                }
            }
            queue.borrow_mut().in_flight = false;
        }
    ));
}

fn schedule_config_apply(queue: &ApplyQueue, cfg: Config) {
    let mut q = queue.borrow_mut();
    q.pending = Some(cfg);
    if let Some(id) = q.debounce.take() {
        id.remove();
    }
    q.debounce = Some(glib::timeout_add_local_once(
        CONFIG_APPLY_DEBOUNCE,
        glib::clone!(
            #[strong]
            queue,
            move || {
                queue.borrow_mut().debounce = None;
                drain_config_apply_queue(&queue);
            }
        ),
    ));
}

fn config_apply_idle(queue: &ApplyQueue) -> bool {
    let q = queue.borrow();
    q.pending.is_none() && q.debounce.is_none() && !q.in_flight
}

fn flush_config_apply(queue: &ApplyQueue) {
    if let Some(id) = queue.borrow_mut().debounce.take() {
        id.remove();
    }
    drain_config_apply_queue(queue);
}

/// Holds the rows populated by the config dialog. GTK widgets are reference counted, so this
/// stores handles instead of borrows and can be shared with the async reload without listing
/// all 25 rows again.
struct ConfigRows {
    inference_execution_provider: libadwaita::ComboRow,
    inference_device: libadwaita::ComboRow,
    level: libadwaita::ComboRow,
    detector: libadwaita::ComboRow,
    recognizer: libadwaita::ComboRow,
    rgb_threshold: libadwaita::SpinRow,
    ir_threshold: libadwaita::SpinRow,
    camera: libadwaita::ComboRow,
    ir: libadwaita::ComboRow,
    ir_frame_size: libadwaita::ExpanderRow,
    ir_frame_width: libadwaita::SpinRow,
    ir_frame_height: libadwaita::SpinRow,
    emitter: gtk4::Switch,
    parallel_capture: libadwaita::ComboRow,
    dark_luma_threshold: libadwaita::SpinRow,
    templates: libadwaita::SpinRow,
    min_face_size_ratio: libadwaita::SpinRow,
    liveness_enabled: gtk4::Switch,
    liveness_threshold: libadwaita::SpinRow,
    liveness_max_seconds: libadwaita::SpinRow,
    require_confirm_lock_screen: gtk4::Switch,
    require_confirm_elevation: gtk4::Switch,
    hybrid: libadwaita::ComboRow,
    abort_ssh: gtk4::Switch,
    abort_lid: gtk4::Switch,
    abort_first_resume: gtk4::Switch,
    resume_grace: libadwaita::SpinRow,
    start_delay: libadwaita::SpinRow,
    start_delay_scope: libadwaita::ComboRow,
    encrypt_templates: gtk4::Switch,
    unlock_gnome_keyring: gtk4::Switch,
    unlock_kwallet: gtk4::Switch,
}

fn commit_focused_spin_row(window: &libadwaita::Window, rows: &ConfigRows) {
    let Some(focus) = gtk4::prelude::GtkWindowExt::focus(window) else {
        return;
    };

    for row in [
        &rows.rgb_threshold,
        &rows.ir_threshold,
        &rows.ir_frame_width,
        &rows.ir_frame_height,
        &rows.dark_luma_threshold,
        &rows.templates,
        &rows.min_face_size_ratio,
        &rows.liveness_threshold,
        &rows.liveness_max_seconds,
        &rows.resume_grace,
        &rows.start_delay,
    ] {
        if focus.is_ancestor(row) {
            row.update();
            return;
        }
    }
}

struct CameraChoices<'a> {
    cameras: &'a [(String, String)],
    ir_options: &'a [(String, String)],
}

fn camera_subtitle(options: &[(String, String)], configured: &str) -> Option<String> {
    if is_listed_source(options, configured) {
        None
    } else {
        Some(format!(
            "Configured as {configured}, which this list cannot show"
        ))
    }
}

fn set_camera_row_subtitle(
    row: &libadwaita::ComboRow,
    options: &[(String, String)],
    configured: &str,
) {
    match camera_subtitle(options, configured) {
        Some(subtitle) => row.set_subtitle(&subtitle),
        None => row.set_subtitle(""),
    }
}

fn populate_config_rows(cfg: &Config, rows: &ConfigRows, choices: CameraChoices<'_>) {
    rows.inference_execution_provider
        .set_selected(cfg.inference.execution_provider_index());
    rows.inference_device
        .set_selected(cfg.inference.device_index());
    if cfg.inference.is_representable() {
        rows.inference_execution_provider
            .set_subtitle("CPU, automatic NPU, Intel OpenVINO, or AMD Ryzen AI");
    } else {
        rows.inference_execution_provider.set_subtitle(&format!(
            "Configured as {}/{}, which this build cannot show",
            cfg.inference.execution_provider, cfg.inference.device
        ));
    }
    set_inference_device_row_visible(&rows.inference_execution_provider, &rows.inference_device);

    rows.level.set_selected(cfg.security.level_index());
    set_custom_config_rows_visible(
        &rows.level,
        &rows.detector,
        &rows.recognizer,
        &rows.rgb_threshold,
        &rows.ir_threshold,
        &rows.hybrid,
    );

    let seed = cfg.security.custom_form();
    rows.detector
        .set_selected(SecurityLevel::model_quality_index(&seed.detector));
    rows.recognizer
        .set_selected(SecurityLevel::model_quality_index(&seed.recognizer));
    rows.rgb_threshold.set_value(seed.rgb_threshold);
    rows.ir_threshold.set_value(seed.ir_threshold);

    let cam_idx = source_index(choices.cameras, &cfg.cameras.rgb);
    rows.camera.set_selected(cam_idx as u32);
    set_camera_row_subtitle(&rows.camera, choices.cameras, &cfg.cameras.rgb);
    let ir_idx = source_index(choices.ir_options, &cfg.cameras.ir);
    rows.ir.set_selected(ir_idx as u32);
    set_camera_row_subtitle(&rows.ir, choices.ir_options, &cfg.cameras.ir);
    let ir_frame_size = cfg.cameras.ir_frame_size();
    if let Some((width, height)) = ir_frame_size {
        rows.ir_frame_width.set_value(width as f64);
        rows.ir_frame_height.set_value(height as f64);
    }
    rows.ir_frame_size
        .set_enable_expansion(ir_frame_size.is_some());
    rows.ir_frame_size
        .set_visible(!cfg.cameras.ir.is_empty() || ir_frame_size.is_some());
    rows.emitter.set_active(cfg.cameras.emitter_enabled);
    rows.parallel_capture
        .set_selected(cfg.cameras.parallel_capture_index());
    rows.dark_luma_threshold
        .set_value(cfg.cameras.dark_luma_threshold as f64);
    rows.templates
        .set_value(cfg.enrollment.max_templates as f64);
    rows.min_face_size_ratio
        .set_value(cfg.enrollment.min_face_size_ratio);
    rows.liveness_enabled.set_active(cfg.liveness.enabled);
    rows.liveness_threshold.set_value(cfg.liveness.threshold);
    rows.liveness_max_seconds
        .set_value(cfg.liveness.max_seconds);
    rows.require_confirm_lock_screen
        .set_active(cfg.auth.require_confirmation_lock_screen);
    rows.require_confirm_elevation
        .set_active(cfg.auth.require_confirmation_elevation);
    rows.hybrid
        .set_selected(SecurityLevel::hybrid_policy_index_for_value(
            &seed.hybrid_policy,
        ));
    rows.abort_ssh.set_active(cfg.auth.abort_if_ssh);
    rows.abort_lid.set_active(cfg.auth.abort_if_lid_closed);
    rows.abort_first_resume
        .set_active(cfg.auth.abort_before_first_resume);
    rows.resume_grace.set_value(cfg.auth.resume_grace_ms as f64);
    rows.start_delay.set_value(cfg.auth.start_delay_ms as f64);
    rows.start_delay_scope
        .set_selected(AuthConfig::start_delay_scope_index_for_value(
            cfg.auth.start_delay_scope(),
        ));
    set_start_delay_scope_row_visible(&rows.start_delay, &rows.start_delay_scope);
    rows.encrypt_templates
        .set_active(cfg.storage.encrypt_templates);
    rows.unlock_gnome_keyring
        .set_active(cfg.storage.unlock_gnome_keyring);
    rows.unlock_kwallet.set_active(cfg.storage.unlock_kwallet);

    set_liveness_config_rows_visible(
        &rows.liveness_enabled,
        &rows.liveness_threshold,
        &rows.liveness_max_seconds,
    );
    set_keyring_config_sensitivity(
        &rows.unlock_gnome_keyring,
        &rows.unlock_kwallet,
        &rows.liveness_enabled,
        &rows.encrypt_templates,
    );
}

pub fn show_config_dialog(
    parent: &libadwaita::ApplicationWindow,
    overlay: &libadwaita::ToastOverlay,
) {
    let config = Rc::new(RefCell::new(Config::default()));

    let window = libadwaita::Window::builder()
        .transient_for(parent)
        .modal(true)
        .title("Settings")
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
    level_row.set_title("Security level");
    level_row.set_subtitle("Adjust the balance between speed and security");
    let level_model = gtk4::StringList::new(&SECURITY_LEVEL_LABELS);
    level_row.set_model(Some(&level_model));
    security_group.add(&level_row);

    let detector_row = libadwaita::ComboRow::new();
    detector_row.set_title("Detector level");
    let detector_model = gtk4::StringList::new(&MODEL_QUALITY_LABELS);
    detector_row.set_model(Some(&detector_model));
    security_group.add(&detector_row);

    let recognizer_row = libadwaita::ComboRow::new();
    recognizer_row.set_title("Recognizer level");
    let recognizer_model = gtk4::StringList::new(&MODEL_QUALITY_LABELS);
    recognizer_row.set_model(Some(&recognizer_model));
    security_group.add(&recognizer_row);

    let make_threshold_row = |title: &str, subtitle: &str| {
        let row = libadwaita::SpinRow::with_range(
            gaze_core::config::MIN_SECURITY_THRESHOLD,
            gaze_core::config::MAX_SECURITY_THRESHOLD,
            0.01,
        );
        row.set_digits(3);
        row.set_title(title);
        row.set_subtitle(subtitle);
        row
    };

    let rgb_threshold_row = make_threshold_row(
        "RGB recognizer threshold",
        "Minimum RGB similarity for a match",
    );
    security_group.add(&rgb_threshold_row);

    let ir_threshold_row = make_threshold_row(
        "IR recognizer threshold",
        "Minimum IR similarity for a match",
    );
    security_group.add(&ir_threshold_row);

    let hardware_group = libadwaita::PreferencesGroup::new();
    hardware_group.set_title("Hardware");
    page.add(&hardware_group);

    let inference_execution_provider_row = libadwaita::ComboRow::new();
    inference_execution_provider_row.set_title("Inference execution provider");
    inference_execution_provider_row
        .set_subtitle("CPU, automatic NPU, Intel OpenVINO, or AMD Ryzen AI");
    let inference_execution_provider_model =
        gtk4::StringList::new(&INFERENCE_EXECUTION_PROVIDER_LABELS);
    inference_execution_provider_row.set_model(Some(&inference_execution_provider_model));
    hardware_group.add(&inference_execution_provider_row);

    let inference_device_row = libadwaita::ComboRow::new();
    inference_device_row.set_title("OpenVINO inference device");
    inference_device_row.set_subtitle("The Intel device used for all ONNX models");
    let inference_device_model = gtk4::StringList::new(&INFERENCE_DEVICE_LABELS);
    inference_device_row.set_model(Some(&inference_device_model));
    hardware_group.add(&inference_device_row);

    let cameras = gaze_vision::camera::rgb_choices().unwrap_or_else(|_| {
        vec![
            ("Primary camera".to_string(), DEFAULT_RGB_CAMERA.to_string()),
            (
                gaze_vision::camera::RGB_NONE_DISPLAY_NAME.to_string(),
                String::new(),
            ),
        ]
    });
    let cam_names = cameras.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();

    let camera_row = libadwaita::ComboRow::new();
    camera_row.set_title("RGB camera source");
    let cam_model =
        gtk4::StringList::new(&cam_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    camera_row.set_model(Some(&cam_model));
    hardware_group.add(&camera_row);

    let ir_options = gaze_vision::camera::ir_choices();
    let ir_names = ir_options
        .iter()
        .map(|(n, _)| n.clone())
        .collect::<Vec<_>>();

    let ir_row = libadwaita::ComboRow::new();
    ir_row.set_title("IR camera source");
    let ir_model = gtk4::StringList::new(&ir_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    ir_row.set_model(Some(&ir_model));
    hardware_group.add(&ir_row);

    let ir_frame_size_row = libadwaita::ExpanderRow::new();
    ir_frame_size_row.set_title("IR frame size override");
    ir_frame_size_row
        .set_subtitle("Force one resolution if the IR feed is green or corrupted on auto");
    ir_frame_size_row.set_show_enable_switch(true);
    ir_frame_size_row.set_enable_expansion(false);
    hardware_group.add(&ir_frame_size_row);

    let ir_frame_width_row =
        libadwaita::SpinRow::with_range(1.0, MAX_IR_FRAME_DIMENSION as f64, 1.0);
    ir_frame_width_row.set_digits(0);
    ir_frame_width_row.set_title("Width");
    ir_frame_width_row.set_value(640.0);
    ir_frame_size_row.add_row(&ir_frame_width_row);

    let ir_frame_height_row =
        libadwaita::SpinRow::with_range(1.0, MAX_IR_FRAME_DIMENSION as f64, 1.0);
    ir_frame_height_row.set_digits(0);
    ir_frame_height_row.set_title("Height");
    ir_frame_height_row.set_value(480.0);
    ir_frame_size_row.add_row(&ir_frame_height_row);

    let emitter_row = libadwaita::ActionRow::new();
    emitter_row.set_title("Force IR emitter");
    emitter_row
        .set_subtitle("Override emitter control, only if the IR emitter stays off on its own");
    let emitter_switch = gtk4::Switch::new();
    emitter_switch.set_valign(gtk4::Align::Center);
    emitter_row.add_suffix(&emitter_switch);
    hardware_group.add(&emitter_row);

    let parallel_capture_row = libadwaita::ComboRow::new();
    parallel_capture_row.set_title("Parallel RGB + IR capture");
    parallel_capture_row.set_subtitle(
        "Faster hybrid authentication, but some webcams cannot stream both sensors at once",
    );
    let parallel_capture_model = gtk4::StringList::new(&PARALLEL_CAPTURE_LABELS);
    parallel_capture_row.set_model(Some(&parallel_capture_model));
    hardware_group.add(&parallel_capture_row);

    let dark_luma_threshold_row = libadwaita::SpinRow::with_range(0.0, 255.0, 1.0);
    dark_luma_threshold_row.set_digits(0);
    dark_luma_threshold_row.set_title("Darkness cutoff");
    dark_luma_threshold_row.set_subtitle("Reject frames below this mean brightness (0–255)");
    hardware_group.add(&dark_luma_threshold_row);

    let enrollment_group = libadwaita::PreferencesGroup::new();
    enrollment_group.set_title("Enrollment");
    page.add(&enrollment_group);

    let templates_row = libadwaita::SpinRow::with_range(1.0, 50.0, 1.0);
    templates_row.set_title("Maximum templates");
    templates_row.set_subtitle("Number of capture sets stored per face");
    enrollment_group.add(&templates_row);

    let min_face_size_ratio_row = libadwaita::SpinRow::with_range(
        MIN_ENROLLMENT_FACE_SIZE_RATIO,
        MAX_ENROLLMENT_FACE_SIZE_RATIO,
        0.01,
    );
    min_face_size_ratio_row.set_digits(2);
    min_face_size_ratio_row.set_title("Minimum face size ratio");
    min_face_size_ratio_row
        .set_subtitle("Smallest accepted face during enrollment; lower allows more distance");
    enrollment_group.add(&min_face_size_ratio_row);

    let liveness_group = libadwaita::PreferencesGroup::new();
    liveness_group.set_title("Liveness Anti-Spoofing");
    page.add(&liveness_group);

    let liveness_enabled_row = libadwaita::ActionRow::new();
    liveness_enabled_row.set_title("Enable liveness spoof prevention");
    liveness_enabled_row.set_subtitle("Analyze face depth/reflectance to prevent photo spoofing");
    let liveness_enabled_switch = gtk4::Switch::new();
    liveness_enabled_switch.set_valign(gtk4::Align::Center);
    liveness_enabled_row.add_suffix(&liveness_enabled_switch);
    liveness_group.add(&liveness_enabled_row);

    let liveness_threshold_row = libadwaita::SpinRow::with_range(
        gaze_core::config::MIN_LIVENESS_THRESHOLD,
        gaze_core::config::MAX_LIVENESS_THRESHOLD,
        0.01,
    );
    liveness_threshold_row.set_digits(3);
    liveness_threshold_row.set_title("Liveness threshold");
    liveness_threshold_row.set_subtitle("Minimum spoof prevention confidence");
    liveness_group.add(&liveness_threshold_row);

    let liveness_max_seconds_row =
        libadwaita::SpinRow::with_range(MIN_LIVENESS_MAX_SECONDS, MAX_LIVENESS_MAX_SECONDS, 0.1);
    liveness_max_seconds_row.set_digits(1);
    liveness_max_seconds_row.set_title("Maximum liveness duration");
    liveness_max_seconds_row.set_subtitle("Maximum seconds analyzed for liveness verification");
    liveness_group.add(&liveness_max_seconds_row);

    let auth_group = libadwaita::PreferencesGroup::new();
    auth_group.set_title("Authentication");
    page.add(&auth_group);

    let abort_ssh_row = libadwaita::ActionRow::new();
    abort_ssh_row.set_title("Abort if SSH");
    abort_ssh_row.set_subtitle("Prevent authentication over SSH connections");
    let abort_ssh_switch = gtk4::Switch::new();
    abort_ssh_switch.set_valign(gtk4::Align::Center);
    abort_ssh_row.add_suffix(&abort_ssh_switch);
    auth_group.add(&abort_ssh_row);

    let abort_lid_row = libadwaita::ActionRow::new();
    abort_lid_row.set_title("Abort if lid closed");
    abort_lid_row.set_subtitle("Prevent authentication when the laptop lid is closed");
    let abort_lid_switch = gtk4::Switch::new();
    abort_lid_switch.set_valign(gtk4::Align::Center);
    abort_lid_row.add_suffix(&abort_lid_switch);
    auth_group.add(&abort_lid_row);

    let abort_first_resume_row = libadwaita::ActionRow::new();
    abort_first_resume_row.set_title("Require a suspend first");
    abort_first_resume_row
        .set_subtitle("Prevent authentication until the system has suspended and resumed once");
    let abort_first_resume_switch = gtk4::Switch::new();
    abort_first_resume_switch.set_valign(gtk4::Align::Center);
    abort_first_resume_row.add_suffix(&abort_first_resume_switch);
    auth_group.add(&abort_first_resume_row);

    let require_confirm_lock_screen_row = libadwaita::ActionRow::new();
    require_confirm_lock_screen_row.set_title("Require confirmation on lock screen");
    require_confirm_lock_screen_row.set_subtitle(
        "Require pressing Enter or clicking OK to authorize after face matches on the lock screen or login screen",
    );
    let require_confirm_lock_screen_switch = gtk4::Switch::new();
    require_confirm_lock_screen_switch.set_valign(gtk4::Align::Center);
    require_confirm_lock_screen_row.add_suffix(&require_confirm_lock_screen_switch);
    auth_group.add(&require_confirm_lock_screen_row);

    let require_confirm_elevation_row = libadwaita::ActionRow::new();
    require_confirm_elevation_row.set_title("Require confirmation for elevated authentication");
    require_confirm_elevation_row.set_subtitle(
        "Require pressing Enter or clicking OK to authorize after face matches for sudo, polkit, and similar prompts",
    );
    let require_confirm_elevation_switch = gtk4::Switch::new();
    require_confirm_elevation_switch.set_valign(gtk4::Align::Center);
    require_confirm_elevation_row.add_suffix(&require_confirm_elevation_switch);
    auth_group.add(&require_confirm_elevation_row);

    let resume_grace_row = libadwaita::SpinRow::with_range(0.0, 10000.0, 100.0);
    resume_grace_row.set_digits(0);
    resume_grace_row.set_title("Resume grace period (ms)");
    resume_grace_row.set_subtitle("Delay face authentication on wake from suspend");
    auth_group.add(&resume_grace_row);

    let start_delay_row = libadwaita::SpinRow::with_range(0.0, 10000.0, 500.0);
    start_delay_row.set_digits(0);
    start_delay_row.set_title("Start delay (ms)");
    start_delay_row.set_subtitle("Delay before face authentication starts");
    auth_group.add(&start_delay_row);

    let start_delay_scope_row = libadwaita::ComboRow::new();
    start_delay_scope_row.set_title("Start delay applies to");
    start_delay_scope_row.set_subtitle("Which prompts wait for the start delay");
    let start_delay_scope_model = gtk4::StringList::new(&START_DELAY_SCOPE_LABELS);
    start_delay_scope_row.set_model(Some(&start_delay_scope_model));
    auth_group.add(&start_delay_scope_row);

    let hybrid_row = libadwaita::ComboRow::new();
    hybrid_row.set_title("Hybrid combining policy");
    hybrid_row.set_subtitle("Combining policy when both RGB and IR cameras are active");
    let hybrid_model = gtk4::StringList::new(&HYBRID_POLICY_LABELS);
    hybrid_row.set_model(Some(&hybrid_model));
    security_group.add(&hybrid_row);

    let storage_group = libadwaita::PreferencesGroup::new();
    storage_group.set_title("Storage");
    page.add(&storage_group);

    let encrypt_templates_row = libadwaita::ActionRow::new();
    encrypt_templates_row.set_title("Encrypt face templates");
    encrypt_templates_row.set_subtitle("Encrypt face templates at rest with a TPM-sealed key");
    let encrypt_templates_switch = gtk4::Switch::new();
    encrypt_templates_switch.set_valign(gtk4::Align::Center);
    encrypt_templates_row.add_suffix(&encrypt_templates_switch);
    storage_group.add(&encrypt_templates_row);

    let unlock_gnome_keyring_row = libadwaita::ActionRow::new();
    // Stays hidden until the load below reports a daemon that can store the flag.
    unlock_gnome_keyring_row.set_visible(false);
    unlock_gnome_keyring_row.set_title("Unlock GNOME Keyring");
    unlock_gnome_keyring_row
        .set_subtitle("Use an enrolled password after liveness-protected GDM or greetd face login");
    let unlock_gnome_keyring_switch = gtk4::Switch::new();
    unlock_gnome_keyring_switch.set_valign(gtk4::Align::Center);
    unlock_gnome_keyring_row.add_suffix(&unlock_gnome_keyring_switch);
    storage_group.add(&unlock_gnome_keyring_row);

    let unlock_kwallet_row = libadwaita::ActionRow::new();
    unlock_kwallet_row.set_visible(false);
    unlock_kwallet_row.set_title("Unlock KWallet");
    unlock_kwallet_row.set_subtitle("Use an enrolled password after liveness-protected KDE login; enroll with gaze keyring --kwallet");
    let unlock_kwallet_switch = gtk4::Switch::new();
    unlock_kwallet_switch.set_valign(gtk4::Align::Center);
    unlock_kwallet_row.add_suffix(&unlock_kwallet_switch);
    storage_group.add(&unlock_kwallet_row);

    liveness_enabled_switch.connect_active_notify(glib::clone!(
        #[weak]
        liveness_threshold_row,
        #[weak]
        liveness_max_seconds_row,
        move |sw| {
            set_liveness_config_rows_visible(
                sw,
                &liveness_threshold_row,
                &liveness_max_seconds_row,
            );
        }
    ));

    for switch in [
        &liveness_enabled_switch,
        &encrypt_templates_switch,
        &unlock_gnome_keyring_switch,
        &unlock_kwallet_switch,
    ] {
        switch.connect_active_notify(glib::clone!(
            #[weak]
            liveness_enabled_switch,
            #[weak]
            encrypt_templates_switch,
            #[weak]
            unlock_gnome_keyring_switch,
            #[weak]
            unlock_kwallet_switch,
            move |_| {
                set_keyring_config_sensitivity(
                    &unlock_gnome_keyring_switch,
                    &unlock_kwallet_switch,
                    &liveness_enabled_switch,
                    &encrypt_templates_switch,
                );
            }
        ));
    }

    let is_loading = Rc::new(Cell::new(true));
    let keyring_supported = Rc::new(Cell::new(false));
    let inference_touched = Rc::new(Cell::new(false));
    let camera_touched = Rc::new(Cell::new(false));
    let ir_touched = Rc::new(Cell::new(false));
    let proxy_cell: SharedProxy = Rc::new(RefCell::new(None));
    let apply_queue = new_apply_queue(
        Rc::new({
            let proxy_cell = proxy_cell.clone();
            let keyring_supported = keyring_supported.clone();
            move |cfg: Config| {
                let proxy_cell = proxy_cell.clone();
                let keyring_supported = keyring_supported.clone();
                Box::pin(async move {
                    let proxy = shared_proxy(&proxy_cell).await?;
                    let result = if keyring_supported.get() {
                        apply_config_with_keyring_to_daemon(&proxy, &cfg).await
                    } else {
                        apply_config_to_daemon(&proxy, &cfg).await
                    };
                    if result.is_err() {
                        // The connection is cached for the life of the dialog, so a daemon that
                        // restarted would fail every later write too, silently losing each edit.
                        *proxy_cell.borrow_mut() = None;
                    }
                    result
                }) as ConfigWriteFuture
            }
        }),
        Rc::new({
            let overlay = overlay.clone();
            move |message: String| {
                overlay.add_toast(libadwaita::Toast::new(&message));
            }
        }),
    );

    let is_authorized = Rc::new(Cell::new(false));
    let config_loaded = Rc::new(Cell::new(false));

    let update_locked_state = glib::clone!(
        #[weak]
        banner,
        #[weak]
        scrolled,
        #[strong]
        is_authorized,
        #[strong]
        config_loaded,
        move || {
            let authorized = is_authorized.get();
            let loaded = config_loaded.get();

            if !authorized {
                banner.set_title("Settings are locked");
                banner.set_button_label(Some("Unlock…"));
            } else if !loaded {
                banner.set_title("Could not read the current configuration from the Gaze daemon");
                banner.set_button_label(Some("Try again"));
            }

            banner.set_revealed(!authorized || !loaded);
            scrolled.set_sensitive(authorized && loaded);
        }
    );

    level_row.connect_selected_notify(glib::clone!(
        #[weak]
        detector_row,
        #[weak]
        recognizer_row,
        #[weak]
        rgb_threshold_row,
        #[weak]
        ir_threshold_row,
        #[weak]
        hybrid_row,
        move |row| {
            set_custom_config_rows_visible(
                row,
                &detector_row,
                &recognizer_row,
                &rgb_threshold_row,
                &ir_threshold_row,
                &hybrid_row,
            );
        }
    ));

    inference_execution_provider_row.connect_selected_notify(glib::clone!(
        #[weak]
        inference_device_row,
        move |row| {
            set_inference_device_row_visible(row, &inference_device_row);
        }
    ));

    let apply_changes = glib::clone!(
        #[strong]
        inference_touched,
        #[weak]
        inference_execution_provider_row,
        #[weak]
        inference_device_row,
        #[weak]
        level_row,
        #[weak]
        detector_row,
        #[weak]
        recognizer_row,
        #[weak]
        rgb_threshold_row,
        #[weak]
        ir_threshold_row,
        #[weak]
        camera_row,
        #[weak]
        ir_row,
        #[weak]
        ir_frame_size_row,
        #[weak]
        ir_frame_width_row,
        #[weak]
        ir_frame_height_row,
        #[weak]
        emitter_switch,
        #[weak]
        parallel_capture_row,
        #[weak]
        dark_luma_threshold_row,
        #[weak]
        templates_row,
        #[weak]
        min_face_size_ratio_row,
        #[weak]
        liveness_enabled_switch,
        #[weak]
        liveness_threshold_row,
        #[weak]
        liveness_max_seconds_row,
        #[weak]
        hybrid_row,
        #[weak]
        require_confirm_lock_screen_switch,
        #[weak]
        require_confirm_elevation_switch,
        #[weak]
        abort_ssh_switch,
        #[weak]
        abort_lid_switch,
        #[weak]
        abort_first_resume_switch,
        #[weak]
        resume_grace_row,
        #[weak]
        start_delay_row,
        #[weak]
        start_delay_scope_row,
        #[weak]
        encrypt_templates_switch,
        #[weak]
        unlock_gnome_keyring_switch,
        #[weak]
        unlock_kwallet_switch,
        #[strong]
        cameras,
        #[strong]
        ir_options,
        #[strong]
        config,
        #[strong]
        is_loading,
        #[strong]
        camera_touched,
        #[strong]
        ir_touched,
        #[strong]
        apply_queue,
        move || {
            if is_loading.get() {
                return;
            }

            let mut cfg = config.borrow_mut();
            if inference_touched.get() || cfg.inference.is_representable() {
                cfg.inference.execution_provider = InferenceConfig::execution_provider_from_index(
                    inference_execution_provider_row.selected() as usize,
                )
                .to_string();
                cfg.inference.device = if cfg.inference.execution_provider == "openvino" {
                    InferenceConfig::device_from_index(inference_device_row.selected() as usize)
                        .to_string()
                } else if matches!(cfg.inference.execution_provider.as_str(), "auto" | "vitis") {
                    "npu".to_string()
                } else {
                    "cpu".to_string()
                };
            }
            let hybrid_idx = hybrid_row.selected() as usize;
            let hybrid_policy = SecurityLevel::hybrid_policy_from_index(hybrid_idx);

            if let Some(level) = SecurityLevel::preset_from_index(level_row.selected() as usize) {
                cfg.security = level;
            } else if level_row.selected() == SecurityLevel::CUSTOM_LEVEL_INDEX {
                let det = SecurityLevel::model_quality_from_index(detector_row.selected() as usize);
                let rec =
                    SecurityLevel::model_quality_from_index(recognizer_row.selected() as usize);
                cfg.security = SecurityLevel::custom_with_thresholds(
                    det.to_string(),
                    rec.to_string(),
                    rgb_threshold_row.value(),
                    ir_threshold_row.value(),
                    hybrid_policy,
                );
            }

            if camera_touched.get() || is_listed_source(&cameras, &cfg.cameras.rgb) {
                let cam_idx = camera_row.selected() as usize;
                if let Some((_, target)) = cameras.get(cam_idx) {
                    cfg.cameras.rgb = target.clone();
                }
            }
            if ir_touched.get() || is_listed_source(&ir_options, &cfg.cameras.ir) {
                let ir_idx = ir_row.selected() as usize;
                if let Some((_, target)) = ir_options.get(ir_idx) {
                    cfg.cameras.ir = target.clone();
                }
            }
            cfg.cameras
                .set_ir_frame_size(ir_frame_size_row.enables_expansion().then(|| {
                    (
                        ir_frame_width_row.value() as u32,
                        ir_frame_height_row.value() as u32,
                    )
                }));
            cfg.cameras.emitter_enabled = emitter_switch.is_active();
            cfg.cameras.parallel_capture =
                CameraConfig::parallel_capture_from_index(parallel_capture_row.selected() as usize);
            cfg.cameras.dark_luma_threshold = dark_luma_threshold_row.value() as u8;
            cfg.enrollment.max_templates = templates_row.value() as u32;
            cfg.enrollment.min_face_size_ratio = min_face_size_ratio_row.value();
            cfg.liveness.enabled = liveness_enabled_switch.is_active();
            cfg.liveness.threshold = liveness_threshold_row.value();
            cfg.liveness.max_seconds = liveness_max_seconds_row.value();
            cfg.auth.require_confirmation_lock_screen =
                require_confirm_lock_screen_switch.is_active();
            cfg.auth.require_confirmation_elevation = require_confirm_elevation_switch.is_active();
            cfg.auth.abort_if_ssh = abort_ssh_switch.is_active();
            cfg.auth.abort_if_lid_closed = abort_lid_switch.is_active();
            cfg.auth.abort_before_first_resume = abort_first_resume_switch.is_active();
            cfg.auth.resume_grace_ms = resume_grace_row.value() as u64;
            cfg.auth.start_delay_ms = start_delay_row.value() as u64;
            cfg.auth.start_delay_scope =
                AuthConfig::start_delay_scope_from_index(start_delay_scope_row.selected() as usize);
            cfg.storage.encrypt_templates = encrypt_templates_switch.is_active();
            cfg.storage.unlock_gnome_keyring = unlock_gnome_keyring_switch.is_active();
            cfg.storage.unlock_kwallet = unlock_kwallet_switch.is_active();

            let cfg_to_apply = cfg.clone();
            drop(cfg);

            schedule_config_apply(&apply_queue, cfg_to_apply);
        }
    );

    inference_execution_provider_row.connect_selected_notify(glib::clone!(
        #[strong]
        apply_changes,
        #[strong]
        inference_touched,
        #[strong]
        is_loading,
        move |_| {
            if !is_loading.get() {
                inference_touched.set(true);
            }
            apply_changes()
        }
    ));
    inference_device_row.connect_selected_notify(glib::clone!(
        #[strong]
        apply_changes,
        #[strong]
        inference_touched,
        #[strong]
        is_loading,
        move |_| {
            if !is_loading.get() {
                inference_touched.set(true);
            }
            apply_changes()
        }
    ));
    camera_row.connect_selected_notify(glib::clone!(
        #[strong]
        apply_changes,
        #[strong]
        camera_touched,
        #[strong]
        is_loading,
        move |row| {
            if !is_loading.get() {
                camera_touched.set(true);
                row.set_subtitle("");
            }
            apply_changes()
        }
    ));
    ir_row.connect_selected_notify(glib::clone!(
        #[weak]
        ir_frame_size_row,
        #[strong]
        apply_changes,
        #[strong]
        ir_touched,
        #[strong]
        is_loading,
        move |row| {
            if !is_loading.get() {
                ir_touched.set(true);
                row.set_subtitle("");
            }
            ir_frame_size_row
                .set_visible(row.selected() != 0 || ir_frame_size_row.enables_expansion());
            apply_changes()
        }
    ));
    ir_frame_size_row.connect_enable_expansion_notify(glib::clone!(
        #[strong]
        apply_changes,
        move |_| apply_changes()
    ));
    start_delay_row.connect_value_notify(glib::clone!(
        #[weak]
        start_delay_scope_row,
        #[strong]
        apply_changes,
        move |row| {
            set_start_delay_scope_row_visible(row, &start_delay_scope_row);
            apply_changes();
        }
    ));

    for row in [
        &level_row,
        &detector_row,
        &recognizer_row,
        &hybrid_row,
        &parallel_capture_row,
        &start_delay_scope_row,
    ] {
        row.connect_selected_notify(glib::clone!(
            #[strong]
            apply_changes,
            move |_| apply_changes()
        ));
    }

    for row in [
        &rgb_threshold_row,
        &ir_threshold_row,
        &ir_frame_width_row,
        &ir_frame_height_row,
        &templates_row,
        &min_face_size_ratio_row,
        &dark_luma_threshold_row,
        &resume_grace_row,
        &liveness_threshold_row,
        &liveness_max_seconds_row,
    ] {
        row.connect_value_notify(glib::clone!(
            #[strong]
            apply_changes,
            move |_| apply_changes()
        ));
    }

    for switch in [
        &emitter_switch,
        &liveness_enabled_switch,
        &abort_ssh_switch,
        &abort_lid_switch,
        &abort_first_resume_switch,
        &require_confirm_lock_screen_switch,
        &require_confirm_elevation_switch,
        &encrypt_templates_switch,
        &unlock_gnome_keyring_switch,
        &unlock_kwallet_switch,
    ] {
        switch.connect_active_notify(glib::clone!(
            #[strong]
            apply_changes,
            move |_| apply_changes()
        ));
    }

    let rows = Rc::new(ConfigRows {
        inference_execution_provider: inference_execution_provider_row.clone(),
        inference_device: inference_device_row.clone(),
        level: level_row.clone(),
        detector: detector_row.clone(),
        recognizer: recognizer_row.clone(),
        rgb_threshold: rgb_threshold_row.clone(),
        ir_threshold: ir_threshold_row.clone(),
        camera: camera_row.clone(),
        ir: ir_row.clone(),
        ir_frame_size: ir_frame_size_row.clone(),
        ir_frame_width: ir_frame_width_row.clone(),
        ir_frame_height: ir_frame_height_row.clone(),
        emitter: emitter_switch.clone(),
        parallel_capture: parallel_capture_row.clone(),
        dark_luma_threshold: dark_luma_threshold_row.clone(),
        templates: templates_row.clone(),
        min_face_size_ratio: min_face_size_ratio_row.clone(),
        liveness_enabled: liveness_enabled_switch.clone(),
        liveness_threshold: liveness_threshold_row.clone(),
        liveness_max_seconds: liveness_max_seconds_row.clone(),
        require_confirm_lock_screen: require_confirm_lock_screen_switch.clone(),
        require_confirm_elevation: require_confirm_elevation_switch.clone(),
        hybrid: hybrid_row.clone(),
        abort_ssh: abort_ssh_switch.clone(),
        abort_lid: abort_lid_switch.clone(),
        abort_first_resume: abort_first_resume_switch.clone(),
        resume_grace: resume_grace_row.clone(),
        start_delay: start_delay_row.clone(),
        start_delay_scope: start_delay_scope_row.clone(),
        encrypt_templates: encrypt_templates_switch.clone(),
        unlock_gnome_keyring: unlock_gnome_keyring_switch.clone(),
        unlock_kwallet: unlock_kwallet_switch.clone(),
    });

    {
        // Cloned, not borrowed across the call: populating a row emits its change signal, and
        // that handler takes `config` mutably.
        let cfg = config.borrow().clone();
        populate_config_rows(
            &cfg,
            &rows,
            CameraChoices {
                cameras: &cameras,
                ir_options: &ir_options,
            },
        );
    }

    window.connect_close_request(glib::clone!(
        #[strong]
        rows,
        #[strong]
        apply_queue,
        move |win| {
            commit_focused_spin_row(win, &rows);
            flush_config_apply(&apply_queue);
            glib::Propagation::Proceed
        }
    ));

    // Retryable, because a daemon that was not answering when the dialog opened may well answer
    // now. Without this the rows stay desensitized for the life of the dialog.
    let load_in_flight = Rc::new(Cell::new(false));
    let load_config: Rc<dyn Fn()> = Rc::new(glib::clone!(
        #[strong]
        load_in_flight,
        #[strong]
        rows,
        #[strong]
        cameras,
        #[strong]
        ir_options,
        #[strong]
        config,
        #[strong]
        is_loading,
        #[strong]
        config_loaded,
        #[strong]
        update_locked_state,
        #[strong]
        proxy_cell,
        #[strong]
        overlay,
        #[weak]
        unlock_gnome_keyring_row,
        #[weak]
        unlock_kwallet_row,
        #[strong]
        keyring_supported,
        move || {
            // A second load repopulating live rows fires every change handler, which writes the
            // reloaded values back over whatever the user typed in between.
            if load_in_flight.get() {
                return;
            }
            load_in_flight.set(true);
            is_loading.set(true);
            glib::MainContext::default().spawn_local(glib::clone!(
                #[strong]
                load_in_flight,
                #[strong]
                rows,
                #[strong]
                cameras,
                #[strong]
                ir_options,
                #[strong]
                config,
                #[strong]
                is_loading,
                #[strong]
                config_loaded,
                #[strong]
                update_locked_state,
                #[strong]
                proxy_cell,
                #[strong]
                overlay,
                #[weak]
                unlock_gnome_keyring_row,
                #[weak]
                unlock_kwallet_row,
                #[strong]
                keyring_supported,
                async move {
                    let load_result = async {
                        let proxy = shared_proxy(&proxy_cell).await?;
                        load_config_with_keyring_from_daemon(&proxy).await
                    }
                    .await;

                    match load_result {
                        Ok((cfg, supported)) => {
                            keyring_supported.set(supported.gnome || supported.kwallet);
                            unlock_gnome_keyring_row.set_visible(supported.gnome);
                            unlock_kwallet_row.set_visible(supported.kwallet);
                            populate_config_rows(
                                &cfg,
                                &rows,
                                CameraChoices {
                                    cameras: &cameras,
                                    ir_options: &ir_options,
                                },
                            );

                            *config.borrow_mut() = cfg;
                            is_loading.set(false);
                            config_loaded.set(true);
                        }
                        Err(e) => {
                            // The next read may use a connection this one just lost.
                            *proxy_cell.borrow_mut() = None;
                            overlay.add_toast(libadwaita::Toast::new(&format!(
                                "Failed to load configuration: {}",
                                e
                            )));
                        }
                    }

                    load_in_flight.set(false);
                    update_locked_state();
                }
            ));
        }
    ));

    banner.connect_button_clicked(glib::clone!(
        #[strong]
        is_authorized,
        #[strong]
        config_loaded,
        #[strong]
        load_config,
        #[strong]
        update_locked_state,
        move |_| {
            // Already unlocked, so the button is offering the other thing the banner can be
            // about: a configuration that could not be read.
            if is_authorized.get() && !config_loaded.get() {
                load_config();
                return;
            }
            glib::MainContext::default().spawn_local(glib::clone!(
                #[strong]
                is_authorized,
                #[strong]
                update_locked_state,
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
                    let subject = match Subject::unix_process(std::process::id()) {
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
                            ALLOW_USER_INTERACTION,
                            "",
                        )
                        .await
                    {
                        Ok(res) => {
                            is_authorized.set(res.is_authorized);
                            update_locked_state();
                        }
                        Err(e) => eprintln!("gaze-gui: polkit CheckAuthorization failed: {e}"),
                    }
                }
            ));
        }
    ));

    glib::MainContext::default().spawn_local(glib::clone!(
        #[strong]
        is_authorized,
        #[strong]
        update_locked_state,
        async move {
            let Ok(conn) = Connection::system().await else {
                return;
            };
            let Ok(authority) = AuthorityProxy::new(&conn).await else {
                return;
            };

            let check_auth = |auth: AuthorityProxy<'static>| async move {
                let subject = Subject::unix_process(std::process::id()).ok()?;

                auth.check_authorization(
                    &subject,
                    "com.gundulabs.gaze.manage-config",
                    &HashMap::new(),
                    NO_INTERACTION,
                    "",
                )
                .await
                .ok()
                .map(|res| res.is_authorized)
            };

            if let Some(allowed) = check_auth(authority.clone()).await {
                is_authorized.set(allowed);
                update_locked_state();
            }

            let Ok(mut changed_stream) = authority.receive_changed().await else {
                return;
            };

            while changed_stream.next().await.is_some() {
                if let Some(allowed) = check_auth(authority.clone()).await {
                    is_authorized.set(allowed);
                    update_locked_state();
                }
            }
        }
    ));

    glib::MainContext::default().spawn_local(glib::clone!(
        #[strong]
        proxy_cell,
        #[strong]
        apply_queue,
        #[strong]
        load_config,
        async move {
            let Ok(proxy) = shared_proxy(&proxy_cell).await else {
                return;
            };
            let gaze = proxy.inner();
            let Ok(properties) = zbus::fdo::PropertiesProxy::builder(gaze.connection())
                .destination(gaze.destination().to_owned())
                .path(gaze.path().to_owned())
                .cache_properties(zbus::proxy::CacheProperties::No)
                .build()
                .await
            else {
                return;
            };
            let Ok(mut changes) = properties.receive_properties_changed().await else {
                return;
            };

            while let Some(change) = changes.next().await {
                let Ok(args) = change.args() else {
                    continue;
                };
                let config_changed = args.changed_properties().contains_key("Config")
                    || args.invalidated_properties().contains(&"Config");
                if config_changed && config_apply_idle(&apply_queue) {
                    load_config();
                }
            }
        }
    ));

    load_config();

    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[derive(Default)]
    struct WriteLog {
        applied: Vec<u64>,
        in_flight: usize,
        max_in_flight: usize,
    }

    fn config_with_grace(resume_grace_ms: u64) -> Config {
        let mut cfg = Config::default();
        cfg.auth.resume_grace_ms = resume_grace_ms;
        cfg
    }

    fn pump_until(condition: impl Fn() -> bool, timeout: Duration) -> bool {
        let context = glib::MainContext::default();
        let start = Instant::now();
        while start.elapsed() < timeout {
            if condition() {
                return true;
            }
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(2));
        }
        condition()
    }

    fn pump_for(duration: Duration) {
        pump_until(|| false, duration);
    }

    fn write_duration_for(resume_grace_ms: u64) -> Duration {
        Duration::from_millis(300u64.saturating_sub(resume_grace_ms / 4))
    }

    type RecordedQueue = (ApplyQueue, Rc<RefCell<WriteLog>>, Rc<RefCell<Vec<String>>>);

    fn recording_queue(fail: bool) -> RecordedQueue {
        let log = Rc::new(RefCell::new(WriteLog::default()));
        let errors = Rc::new(RefCell::new(Vec::new()));

        let queue = new_apply_queue(
            Rc::new({
                let log = log.clone();
                move |cfg: Config| {
                    let log = log.clone();
                    Box::pin(async move {
                        {
                            let mut log = log.borrow_mut();
                            log.in_flight += 1;
                            log.max_in_flight = log.max_in_flight.max(log.in_flight);
                        }
                        glib::timeout_future(write_duration_for(cfg.auth.resume_grace_ms)).await;
                        {
                            let mut log = log.borrow_mut();
                            log.in_flight -= 1;
                            log.applied.push(cfg.auth.resume_grace_ms);
                        }
                        if fail {
                            anyhow::bail!("daemon rejected the config");
                        }
                        Ok(())
                    }) as ConfigWriteFuture
                }
            }),
            Rc::new({
                let errors = errors.clone();
                move |message: String| errors.borrow_mut().push(message)
            }),
        );

        (queue, log, errors)
    }

    #[test]
    fn config_applies_are_debounced_and_serialized() {
        let (queue, log, errors) = recording_queue(false);

        for value in [100, 200, 300, 400, 500] {
            schedule_config_apply(&queue, config_with_grace(value));
        }

        assert!(
            pump_until(|| log.borrow().applied.len() == 1, Duration::from_secs(5)),
            "a burst of edits should produce exactly one write"
        );
        assert_eq!(
            log.borrow().applied,
            vec![500],
            "the write should carry the final value, not an intermediate one"
        );

        pump_for(Duration::from_millis(600));
        assert_eq!(log.borrow().applied.len(), 1, "no trailing duplicate write");

        schedule_config_apply(&queue, config_with_grace(600));
        let flushed_at = Instant::now();
        flush_config_apply(&queue);
        assert!(
            pump_until(|| log.borrow().in_flight == 1, Duration::from_millis(200)),
            "flush should start the write without waiting out the debounce"
        );
        assert!(flushed_at.elapsed() < CONFIG_APPLY_DEBOUNCE);

        schedule_config_apply(&queue, config_with_grace(700));
        schedule_config_apply(&queue, config_with_grace(800));

        assert!(
            pump_until(|| log.borrow().applied.len() == 3, Duration::from_secs(5)),
            "edits made during an in-flight write should be applied afterwards"
        );
        assert_eq!(
            log.borrow().applied,
            vec![500, 600, 800],
            "queued edits collapse to the latest value"
        );
        assert_eq!(
            log.borrow().max_in_flight,
            1,
            "writes must never overlap, or the daemon can persist them out of order"
        );
        assert!(errors.borrow().is_empty());

        let (queue, log, errors) = recording_queue(true);
        schedule_config_apply(&queue, config_with_grace(900));
        flush_config_apply(&queue);
        assert!(
            pump_until(|| !errors.borrow().is_empty(), Duration::from_secs(5)),
            "a failed write should be reported"
        );
        assert!(errors.borrow()[0].starts_with("Failed to apply config"));

        schedule_config_apply(&queue, config_with_grace(1000));
        flush_config_apply(&queue);
        assert!(
            pump_until(|| log.borrow().applied.len() == 2, Duration::from_secs(5)),
            "the queue should keep accepting writes after a failure"
        );
    }

    #[test]
    fn custom_security_level_reveals_the_detail_rows() {
        assert!(custom_rows_visible(SecurityLevel::CUSTOM_LEVEL_INDEX));
        assert!(!custom_rows_visible(0));
        assert!(!custom_rows_visible(
            SecurityLevel::CUSTOM_LEVEL_INDEX + 100
        ));
    }

    #[test]
    fn start_delay_scope_only_matters_for_positive_delays() {
        assert!(!start_delay_scope_visible(0.0));
        assert!(!start_delay_scope_visible(-5.0));
        assert!(start_delay_scope_visible(0.5));
        assert!(start_delay_scope_visible(2500.0));
    }

    #[test]
    fn liveness_rows_follow_the_enabled_switch() {
        assert!(liveness_rows_visible(true));
        assert!(!liveness_rows_visible(false));
    }

    #[test]
    fn keyring_switches_need_both_liveness_and_encryption() {
        // Nothing on: everything sensitive except the locks on liveness/encryption.
        assert_eq!(
            keyring_sensitivity(false, false, false, false),
            (false, false, true, true)
        );
        // Only one prerequisite: still locked.
        assert_eq!(
            keyring_sensitivity(false, false, true, false),
            (false, false, true, true)
        );
        assert_eq!(
            keyring_sensitivity(false, false, false, true),
            (false, false, true, true)
        );
        // Both prerequisites: the backends unlock, and they pin liveness/encryption on.
        assert_eq!(
            keyring_sensitivity(false, false, true, true),
            (true, true, true, true)
        );
        // An active backend stays sensitive and locks its prerequisites on.
        assert_eq!(
            keyring_sensitivity(true, false, false, false),
            (true, false, false, false)
        );
        assert_eq!(
            keyring_sensitivity(false, true, false, false),
            (false, true, false, false)
        );
    }

    #[test]
    fn inference_device_row_is_an_openvino_extra() {
        assert!(inference_device_visible("openvino"));
        for provider in ["cpu", "auto", "vitis", "", "OpenVINO"] {
            assert!(
                !inference_device_visible(provider),
                "{provider:?} must not show the device row"
            );
        }
    }

    #[test]
    fn camera_subtitle_is_empty_for_listed_sources() {
        let options = vec![
            ("Primary camera".to_string(), "primary".to_string()),
            ("USB camera".to_string(), "/dev/video0".to_string()),
        ];
        assert_eq!(camera_subtitle(&options, "primary"), None);
        assert_eq!(camera_subtitle(&options, "/dev/video0"), None);
        let unlisted =
            camera_subtitle(&options, "/dev/video9").expect("unlisted source must explain itself");
        assert!(
            unlisted.contains("/dev/video9"),
            "subtitle must name the configured value: {unlisted}"
        );
        assert!(
            camera_subtitle(&[], "anything").is_some(),
            "an empty picker cannot show anything"
        );
    }
}
