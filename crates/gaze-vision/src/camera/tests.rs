// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use gaze_core::config::CameraConfig;

#[test]
fn truncated_camera_buffer_is_rejected_before_opencv_borrows_it() {
    gstreamer::init().unwrap();
    let info = gstreamer_video::VideoInfo::builder(gstreamer_video::VideoFormat::Bgr, 2, 2)
        .build()
        .unwrap();
    let buffer = gstreamer::Buffer::from_mut_slice(vec![0u8; 1]);
    let err = mirrored_bgr_frame(&buffer, &info).unwrap_err();
    assert!(err.to_string().contains("smaller"), "{err}");
}

#[test]
fn camera_metadata_cannot_supply_a_negative_short_or_overflowing_layout() {
    gstreamer::init().unwrap();
    let info = gstreamer_video::VideoInfo::builder(gstreamer_video::VideoFormat::Bgr, 2, 2)
        .build()
        .unwrap();
    for (offset, stride) in [(0, -8), (0, 3), (usize::MAX, 8), (16, 8)] {
        let mut buffer = gstreamer::Buffer::from_mut_slice(vec![0u8; 16]);
        gstreamer_video::VideoMeta::add_full(
            buffer.get_mut().unwrap(),
            gstreamer_video::VideoFrameFlags::empty(),
            gstreamer_video::VideoFormat::Bgr,
            2,
            2,
            &[offset],
            &[stride],
        )
        .unwrap();
        assert!(
            mirrored_bgr_frame(&buffer, &info).is_err(),
            "{offset}/{stride}"
        );
    }
}

#[test]
fn camera_frames_honor_metadata_offset_and_stride_and_copy_the_pixels() {
    gstreamer::init().unwrap();
    let info = gstreamer_video::VideoInfo::builder(gstreamer_video::VideoFormat::Bgr, 2, 2)
        .build()
        .unwrap();
    let mut buffer = gstreamer::Buffer::from_mut_slice(vec![
        99, 99, 99, 99, 99, 99, 99, 99, 1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0,
    ]);
    gstreamer_video::VideoMeta::add_full(
        buffer.get_mut().unwrap(),
        gstreamer_video::VideoFrameFlags::empty(),
        gstreamer_video::VideoFormat::Bgr,
        2,
        2,
        &[8],
        &[8],
    )
    .unwrap();
    let frame = mirrored_bgr_frame(&buffer, &info).unwrap();
    drop(buffer);
    assert_eq!(
        frame_to_bytes(&frame).unwrap(),
        [4, 5, 6, 1, 2, 3, 10, 11, 12, 7, 8, 9]
    );
}

#[test]
fn pipewire_elements_take_the_bound_descriptor() {
    assert_eq!(bind_pipewire_fd("pipewiresrc", 7), "pipewiresrc fd=7");
    assert_eq!(
        bind_pipewire_fd("pipewiresrc target-object=cam1", 7),
        "pipewiresrc target-object=cam1 fd=7"
    );
}

#[test]
fn a_caller_supplied_descriptor_is_not_doubled() {
    assert_eq!(bind_pipewire_fd("pipewiresrc fd=5", 7), "pipewiresrc fd=5");
}

#[test]
fn a_held_device_is_named_as_busy() {
    assert!(device_is_busy(
        "Could not open device: Device or resource busy"
    ));
    assert!(device_is_busy("v4l2 returned EBUSY"));
    assert!(device_is_busy(
        "The camera is already in use by another program (primary): \
             Device '/dev/video0' is busy"
    ));
    assert!(!device_is_busy("No such file or directory"));
}

#[test]
fn a_briefly_busy_node_is_opened_on_retry() {
    let mut calls = 0;
    let result = retry_while_busy("/dev/video0", std::time::Duration::ZERO, || {
        calls += 1;
        if calls < PRIVILEGED_BUSY_ATTEMPTS {
            anyhow::bail!("Device '/dev/video0' is busy")
        }
        Ok(calls)
    });
    assert_eq!(result.unwrap(), PRIVILEGED_BUSY_ATTEMPTS);
}

#[test]
fn a_node_that_stays_busy_fails_after_the_last_attempt() {
    let mut calls = 0;
    let result: anyhow::Result<()> =
        retry_while_busy("/dev/video0", std::time::Duration::ZERO, || {
            calls += 1;
            anyhow::bail!("Device '/dev/video0' is busy")
        });
    assert!(device_is_busy(&format!("{:#}", result.unwrap_err())));
    assert_eq!(calls, PRIVILEGED_BUSY_ATTEMPTS);
}

#[test]
fn other_open_errors_are_not_retried() {
    let mut calls = 0;
    let result: anyhow::Result<()> =
        retry_while_busy("/dev/video0", std::time::Duration::ZERO, || {
            calls += 1;
            anyhow::bail!("No such file or directory")
        });
    assert!(result.is_err());
    assert_eq!(calls, 1);
}

#[test]
fn non_pipewire_elements_are_left_alone() {
    // A V4L2 node has no PipeWire connection to inherit, and `fd=` means something else
    // entirely on other elements.
    assert_eq!(
        bind_pipewire_fd("v4l2src device=/dev/video0", 7),
        "v4l2src device=/dev/video0"
    );
    assert_eq!(bind_pipewire_fd("videotestsrc", 7), "videotestsrc");
    // Guard against matching an element that merely starts with the same letters.
    assert_eq!(bind_pipewire_fd("pipewiresrcfoo", 7), "pipewiresrcfoo");
}

#[test]
fn a_missing_socket_is_reported_rather_than_bound() {
    assert!(PipeWireSession::connect_for_uid(u32::MAX).is_err());
}

#[test]
fn only_an_untargeted_pipewire_source_falls_back_to_any_camera() {
    assert_eq!(v4l2_fallback_for("pipewiresrc"), V4l2Fallback::AnyDevice);
    assert_eq!(
        v4l2_fallback_for("v4l2src device=/dev/video0"),
        V4l2Fallback::None
    );
    assert_eq!(v4l2_fallback_for("pipewiresrcfoo"), V4l2Fallback::None);
    // A PipeWire source with no target still means "any camera", but it is not the bare
    // element, so it takes the field-parsing path.
    assert_eq!(v4l2_fallback_for("pipewiresrc fd=7"), V4l2Fallback::None);
}

#[test]
fn a_pinned_target_falls_back_only_to_its_own_node() {
    assert_eq!(
        v4l2_fallback_for("pipewiresrc target-object=cam1"),
        V4l2Fallback::SameNode("cam1".to_string())
    );
    // The quoting and the trailing fields belong to the element, not to the target name.
    assert_eq!(
        v4l2_fallback_for("pipewiresrc target-object=\"cam1\" fd=7"),
        V4l2Fallback::SameNode("cam1".to_string())
    );
    assert_eq!(
        v4l2_fallback_for("pipewiresrc target-object=v4l2_input.pci-0000_00_14.0-usb-0_7_1.0"),
        V4l2Fallback::SameNode("v4l2_input.pci-0000_00_14.0-usb-0_7_1.0".to_string())
    );
    assert_eq!(
        v4l2_fallback_for("pipewiresrc target-object="),
        V4l2Fallback::None
    );
}

#[test]
fn a_pipewire_target_names_the_udev_path_of_its_node() {
    assert_eq!(
        v4l2_by_path_for_target("v4l2_input.pci-0000_00_14.0-usb-0_7_1.0").as_deref(),
        Some("pci-0000:00:14.0-usb-0:7:1.0")
    );
    // Only V4L2 nodes are linked by path; anything else has no node to fall back to.
    assert_eq!(v4l2_by_path_for_target("alsa_input.pci-0000_00_1f.3"), None);
    assert_eq!(v4l2_by_path_for_target("51"), None);
    assert_eq!(v4l2_by_path_for_target("v4l2_input."), None);
}

#[test]
fn a_deduplicated_pipewire_target_falls_back_to_its_udev_path() {
    let by_path = v4l2_by_path_for_target("v4l2_input.pci-0000_0d_00.4-usb-0_1_1.0.2").unwrap();
    assert_eq!(by_path, "pci-0000:0d:00.4-usb-0:1:1.0.2");
    assert_eq!(
        without_dedup_suffix(&by_path),
        Some("pci-0000:0d:00.4-usb-0:1:1.0")
    );
    assert_eq!(without_dedup_suffix("platform-camera"), None);
    assert_eq!(without_dedup_suffix("pci-0000:0d:00.4-usb-0:1:1.x"), None);
}

#[test]
fn a_failing_stream_keeps_the_reason_it_ended() {
    // A failing element pushes EOS as well as posting the error, so the reason survives
    // only if the bus is read first.
    let mut camera =
        Camera::open("videotestsrc ! identity error-after=3").expect("identity pipeline");
    let stop = AtomicBool::new(false);
    while camera.next_interruptible(&stop).is_some() {}

    let reason = camera
        .take_stream_error()
        .expect("a pipeline error must be kept");
    assert!(
        reason.contains("Failed after iterations as requested"),
        "unexpected reason: {reason}"
    );
    assert!(
        camera.take_stream_error().is_none(),
        "the reason is taken, so it is reported once"
    );
}

#[test]
fn a_clean_end_of_stream_leaves_no_reason() {
    let mut camera = Camera::open("videotestsrc num-buffers=2").expect("videotestsrc pipeline");
    let stop = AtomicBool::new(false);
    while camera.next_interruptible(&stop).is_some() {}

    assert!(
        camera.take_stream_error().is_none(),
        "an ordinary EOS is not a failure"
    );
}

#[test]
fn a_pinned_thread_keeps_its_session_when_the_default_moves() {
    // A capture thread outlives the claim that started it, and the next claim rebinds the
    // process-wide default. What the thread opens must not follow it.
    let (tx, rx) = std::sync::mpsc::channel();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let worker = std::thread::spawn(move || {
        bind_pipewire_uid_for_thread(Some(1000));
        tx.send(current_pipewire_uid()).unwrap();
        go_rx.recv().unwrap();
        tx.send(current_pipewire_uid()).unwrap();
    });

    assert_eq!(rx.recv().unwrap(), Some(1000));
    set_pipewire_uid(Some(1001));
    go_tx.send(()).unwrap();
    assert_eq!(rx.recv().unwrap(), Some(1000));
    worker.join().unwrap();

    // An unpinned thread still follows the default, which is what a claim sets it for.
    assert_eq!(
        std::thread::spawn(current_pipewire_uid).join().unwrap(),
        Some(1001)
    );
    set_pipewire_uid(None);
}

fn entry(display_name: &str, target: &str, node: Option<&str>) -> CameraEntry {
    CameraEntry {
        display_name: display_name.to_string(),
        target: format!("pipewiresrc target-object={target}"),
        node: node.map(str::to_string),
    }
}

#[test]
fn shared_display_names_are_labelled_with_their_v4l2_node() {
    let labelled = label_camera_entries(vec![
        entry("ASUS 5M webcam (V4L2)", "cam0", Some("/dev/video0")),
        entry("ASUS 5M webcam (V4L2)", "cam2", Some("/dev/video2")),
    ]);

    assert_eq!(
        labelled
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec![
            "ASUS 5M webcam (V4L2) (/dev/video0)",
            "ASUS 5M webcam (V4L2) (/dev/video2)"
        ]
    );
    assert_eq!(labelled[1].1, "pipewiresrc target-object=cam2");
}

#[test]
fn a_unique_display_name_is_left_alone() {
    let labelled = label_camera_entries(vec![
        entry("Integrated Webcam", "cam0", Some("/dev/video0")),
        entry("IR Camera", "cam2", Some("/dev/video2")),
    ]);

    assert_eq!(labelled[0].0, "Integrated Webcam");
    assert_eq!(labelled[1].0, "IR Camera");
}

#[test]
fn shared_display_names_without_a_node_stay_as_they_are() {
    let labelled = label_camera_entries(vec![
        entry("ASUS 5M webcam (V4L2)", "cam0", None),
        entry("ASUS 5M webcam (V4L2)", "cam2", Some("/dev/video2")),
    ]);

    assert_eq!(labelled[0].0, "ASUS 5M webcam (V4L2)");
    assert_eq!(labelled[1].0, "ASUS 5M webcam (V4L2) (/dev/video2)");
}

#[test]
fn realtek_ir_pipeline_forces_required_uncompressed_mode() {
    let pipeline = camera_pipeline("v4l2src device=/dev/video0", true, None);
    assert!(pipeline.contains("video/x-raw,format=YUY2,width=640,height=480"));
    assert!(!pipeline.contains("image/jpeg"));

    let rgb_pipeline = camera_pipeline("v4l2src device=/dev/video0", false, None);
    assert!(rgb_pipeline.contains("image/jpeg"));
}

#[test]
fn forced_ir_yuy2_follows_the_device_profile_flag() {
    // Both single-node Realtek modules are flagged in ir-profiles/*.toml.
    assert!(requires_forced_ir_yuy2(0x0bda, 0x5767, false));
    assert!(requires_forced_ir_yuy2(0x0bda, 0x58c2, false));

    // The quirk is IR-only, and never applies to unflagged or unknown devices.
    assert!(!requires_forced_ir_yuy2(0x0bda, 0x58c2, true));
    assert!(!requires_forced_ir_yuy2(0x046d, 0x085e, false));
    assert!(!requires_forced_ir_yuy2(0xdead, 0xbeef, false));
}

#[test]
fn configured_sources_leave_ir_empty_when_no_ir_configured() {
    let cameras = CameraConfig {
        rgb: "primary".to_string(),
        ir: String::new(),
        ir_frame_width: None,
        ir_frame_height: None,
        emitter_enabled: false,
        dark_luma_threshold: 30,
        parallel_capture: "never".to_string(),
        auto_rotate: true,
    };
    let sources = resolve_configured_sources(&cameras);
    assert_eq!(sources.rgb, "primary");
    assert_eq!(sources.ir, "");
    assert_eq!(sources.ir_node, "");
    assert_eq!(
        preferred_capture_source(&cameras),
        ("primary".to_string(), false)
    );
}

fn cameras_with(rgb: &str, ir: &str) -> CameraConfig {
    CameraConfig {
        rgb: rgb.to_string(),
        ir: ir.to_string(),
        ir_frame_width: None,
        ir_frame_height: None,
        emitter_enabled: false,
        dark_luma_threshold: 30,
        parallel_capture: "never".to_string(),
        auto_rotate: true,
    }
}

#[test]
fn two_nodes_on_one_hardware_function_must_capture_one_at_a_time() {
    assert!(functions_must_serialize(
        Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0".to_string()),
        Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0".to_string()),
    ));
}

#[test]
fn separate_hardware_functions_may_stream_concurrently() {
    assert!(!functions_must_serialize(
        Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0".to_string()),
        Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.2".to_string()),
    ));
}

#[test]
fn an_unreadable_hardware_function_keeps_the_serial_path() {
    let known = || Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0".to_string());
    assert!(functions_must_serialize(None, known()));
    assert!(functions_must_serialize(known(), None));
    assert!(functions_must_serialize(None, None));
}

#[test]
fn a_single_node_rgb_ir_module_never_captures_in_parallel() {
    assert!(capture_must_serialize(
        Some("/dev/video0"),
        Some("/dev/video0")
    ));
}

#[test]
fn an_unresolvable_capture_node_falls_back_to_serial() {
    assert!(capture_must_serialize(Some("/dev/video0"), None));
    assert!(capture_must_serialize(None, None));
    assert!(capture_must_serialize(
        Some("/dev/video-absent-rgb"),
        Some("/dev/video-absent-ir")
    ));
}

#[test]
fn a_primary_rgb_source_serializes_against_a_dual_sensor_ir_function() {
    let ir_function = "/sys/devices/pci0000:00/usb1/1-5/1-5:1.0";
    assert!(serialize_for_unresolved_rgb(
        &[ir_function.to_string()],
        Some(ir_function)
    ));
}

#[test]
fn a_primary_rgb_source_streams_beside_an_ir_only_function() {
    assert!(!serialize_for_unresolved_rgb(
        &["/sys/devices/pci0000:00/usb1/1-3/1-3:1.0".to_string()],
        Some("/sys/devices/pci0000:00/usb1/1-5/1-5:1.0")
    ));
}

#[test]
fn a_primary_rgb_source_stays_serial_when_nothing_enumerates() {
    let ir_function = "/sys/devices/pci0000:00/usb1/1-5/1-5:1.0";
    assert!(serialize_for_unresolved_rgb(&[], Some(ir_function)));
    assert!(serialize_for_unresolved_rgb(
        &[ir_function.to_string()],
        None
    ));
    assert!(serialize_for_unresolved_rgb(&[], None));
}

#[test]
fn always_and_never_skip_hardware_detection_entirely() {
    let mut cameras = cameras_with("primary", "/dev/video2");

    cameras.parallel_capture = "never".to_string();
    assert!(resolve_configured_sources(&cameras).serial_capture);

    cameras.parallel_capture = "always".to_string();
    assert!(!resolve_configured_sources(&cameras).serial_capture);
}

#[test]
fn an_unrecognised_capture_mode_stays_serial() {
    let mut cameras = cameras_with("primary", "/dev/video2");
    cameras.parallel_capture = "sometimes".to_string();
    assert!(resolve_configured_sources(&cameras).serial_capture);
}

#[test]
fn configured_sources_resolve_an_ir_device_node() {
    let cameras = CameraConfig {
        rgb: "primary".to_string(),
        ir: "/dev/video2".to_string(),
        ir_frame_width: None,
        ir_frame_height: None,
        emitter_enabled: true,
        dark_luma_threshold: 30,
        parallel_capture: "never".to_string(),
        auto_rotate: true,
    };
    let sources = resolve_configured_sources(&cameras);
    assert_eq!(sources.rgb, "primary");
    assert_eq!(sources.ir, "/dev/video2");
    assert_eq!(sources.ir_node, "/dev/video2");
}

#[test]
fn preferred_capture_source_falls_back_to_ir_when_rgb_is_unset() {
    let cameras = CameraConfig {
        rgb: String::new(),
        ir: "/dev/video2".to_string(),
        ir_frame_width: None,
        ir_frame_height: None,
        emitter_enabled: true,
        dark_luma_threshold: 30,
        parallel_capture: "never".to_string(),
        auto_rotate: true,
    };
    assert_eq!(
        preferred_capture_source(&cameras),
        ("/dev/video2".to_string(), true)
    );
}

#[test]
fn open_scales_widescreen_to_square_pixels() {
    let mut camera = Camera::open(
        "videotestsrc num-buffers=3 ! capsfilter caps=video/x-raw,width=1280,height=720",
    )
    .expect("videotestsrc pipeline");
    let frame = camera.next().expect("videotestsrc frame");
    assert_eq!(frame.rows(), 480);
    // Without the PAR pin videoscale keeps the source width (1280x480 at PAR 2/3).
    let cols = frame.cols();
    assert!(
        (853..=854).contains(&cols),
        "expected aspect-preserving width, got {cols}"
    );
}

#[test]
fn iterator_ends_after_eos() {
    let mut camera = Camera::open("videotestsrc num-buffers=2").expect("videotestsrc pipeline");
    assert!(camera.next().is_some());
    assert!(camera.next().is_some());
    assert!(camera.next().is_none(), "iterator must end at EOS");
}

#[test]
fn iterator_ends_on_pipeline_error() {
    let mut camera = Camera::open("videotestsrc is-live=true ! identity error-after=5")
        .expect("identity pipeline");
    let mut frames = 0;
    while camera.next().is_some() {
        frames += 1;
        assert!(frames < 10, "iterator must end after the pipeline errors");
    }
}

#[test]
fn interruptible_read_stops_when_live_pipeline_has_no_frames() {
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);

    let worker = std::thread::spawn(move || {
        let mut camera =
            Camera::open("appsrc is-live=true format=time").expect("live pipeline without frames");
        ready_tx.send(()).expect("signal camera readiness");
        let stopped = camera.next_interruptible(&worker_stop).is_none();
        done_tx.send(stopped).expect("signal camera shutdown");
    });

    ready_rx
        .recv_timeout(std::time::Duration::from_secs(2))
        .expect("camera must become ready");
    std::thread::sleep(std::time::Duration::from_millis(25));
    stop.store(true, Ordering::Release);
    assert!(
        done_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("camera read must observe cancellation")
    );
    worker.join().expect("camera worker must exit");
}

#[test]
fn classify_source_maps_every_supported_form() {
    assert_eq!(
        classify_source("primary", true).unwrap(),
        SourceElement::Element("pipewiresrc".to_string())
    );
    assert_eq!(
        classify_source("/dev/video0", true).unwrap(),
        SourceElement::Element("v4l2src device=/dev/video0".to_string())
    );
    assert_eq!(
        classify_source("/dev/video2", false).unwrap(),
        SourceElement::Element("v4l2src device=/dev/video2".to_string())
    );
    assert_eq!(
        classify_source("usb:046d:085e", true).unwrap(),
        SourceElement::ResolveUsb {
            vid: 0x046d,
            pid: 0x085e,
            want_color: true,
        }
    );
    assert_eq!(
        classify_source("usb:046d:085e", false).unwrap(),
        SourceElement::ResolveUsb {
            vid: 0x046d,
            pid: 0x085e,
            want_color: false,
        }
    );
    assert_eq!(
        classify_source("v4l2src device=/dev/video0", true).unwrap(),
        SourceElement::Element("v4l2src device=/dev/video0".to_string())
    );
}

#[test]
fn classify_source_rejects_malformed_values() {
    assert!(classify_source("", true).is_err());
    assert!(classify_source("/dev/video", true).is_err());
    assert!(classify_source("/dev/video2 ! fakesink", true).is_err());
    assert!(classify_source("usb:046d", true).is_err());
    assert!(classify_source("usb:zzzz:085e", true).is_err());
}

#[test]
fn open_surfaces_malformed_sources() {
    assert!(Camera::open("/dev/video").is_err());
    assert!(Camera::open_ir("/dev/video2 ! fakesink", None).is_err());
    assert!(Camera::open("usb:046d").is_err());
}

#[test]
fn parse_usb_spec_reads_hex_vid_pid() {
    assert_eq!(parse_usb_spec("usb:046d:085e"), Some((0x046d, 0x085e)));
    assert_eq!(parse_usb_spec("usb:04F2:B67C"), Some((0x04f2, 0xb67c)));
    assert_eq!(parse_usb_spec("pci:046d:085e"), None);
    assert_eq!(parse_usb_spec("usb:046d"), None);
    assert_eq!(parse_usb_spec("usb:zzzz:085e"), None);
    assert_eq!(parse_usb_spec("primary"), None);
}

#[test]
fn select_usb_node_disambiguates_color_and_mono() {
    // Brio-style single-function UVC: one VID:PID exposes a color node and a
    // mono IR node. Also a second, color-only device.
    let nodes = vec![
        VideoNodeInfo {
            node: "/dev/video0".to_string(),
            vid: 0x046d,
            pid: 0x085e,
            is_color: true,
        },
        VideoNodeInfo {
            node: "/dev/video2".to_string(),
            vid: 0x046d,
            pid: 0x085e,
            is_color: false,
        },
        VideoNodeInfo {
            node: "/dev/video4".to_string(),
            vid: 0x1234,
            pid: 0x5678,
            is_color: true,
        },
    ];
    assert_eq!(
        select_usb_node(&nodes, 0x046d, 0x085e, true),
        Some("/dev/video0".to_string())
    );
    assert_eq!(
        select_usb_node(&nodes, 0x046d, 0x085e, false),
        Some("/dev/video2".to_string())
    );
    // No mono node on a color-only device, and unknown ids resolve to nothing.
    assert_eq!(select_usb_node(&nodes, 0x1234, 0x5678, false), None);
    assert_eq!(select_usb_node(&nodes, 0xdead, 0xbeef, true), None);
}

#[test]
fn select_usb_node_prefers_lowest_numbered_node() {
    let nodes = vec![
        VideoNodeInfo {
            node: "/dev/video10".to_string(),
            vid: 0x046d,
            pid: 0x085e,
            is_color: true,
        },
        VideoNodeInfo {
            node: "/dev/video2".to_string(),
            vid: 0x046d,
            pid: 0x085e,
            is_color: true,
        },
    ];
    assert_eq!(
        select_usb_node(&nodes, 0x046d, 0x085e, true),
        Some("/dev/video2".to_string())
    );
}

#[test]
fn select_usb_node_reuses_single_node_for_known_ir_profile() {
    let nodes = vec![VideoNodeInfo {
        node: "/dev/video6".to_string(),
        vid: 0x0bda,
        pid: 0x58c2,
        is_color: true,
    }];

    assert_eq!(
        select_usb_node(&nodes, 0x0bda, 0x58c2, false),
        Some("/dev/video6".to_string())
    );
}

#[test]
fn configured_sources_leave_the_node_empty_for_a_pipewire_ir_source() {
    let cameras = CameraConfig {
        rgb: "primary".to_string(),
        ir: "pipewiresrc target-object=device-name".to_string(),
        ir_frame_width: None,
        ir_frame_height: None,
        emitter_enabled: true,
        dark_luma_threshold: 30,
        parallel_capture: "auto".to_string(),
        auto_rotate: true,
    };
    let sources = resolve_configured_sources(&cameras);
    assert_eq!(sources.ir, "pipewiresrc target-object=device-name");
    assert_eq!(sources.ir_node, "");
    assert!(
        sources.serial_capture,
        "an unresolvable node must keep the safe serial capture path"
    );
}

#[test]
fn mono_format_detection_is_case_and_whitespace_insensitive() {
    for format in [
        "GRAY8", " gray16 ", "GREY", "grey12", "R8", "r16", "Y8", " y16 ",
    ] {
        assert!(is_mono_format(format), "{format} should be mono");
    }

    for format in ["RGB", "BGR", "RGBA", "YUY2", "NV12", "DMA_DRM", ""] {
        assert!(!is_mono_format(format), "{format} should be color/unknown");
    }
}

#[test]
fn privileged_capture_keeps_canonical_kernel_nodes() {
    assert_eq!(
        resolve_privileged_node("/dev/video0", true),
        Some("/dev/video0".to_string())
    );
    assert_eq!(
        resolve_privileged_node("  /dev/video2  ", false),
        Some("/dev/video2".to_string())
    );
}

#[test]
fn privileged_capture_rejects_untrusted_or_malformed_sources() {
    for source in [
        "",
        "videotestsrc num-buffers=2",
        "v4l2src device=/dev/video0",
        "/dev/video",
        "/dev/video2 ! fakesink",
        "/dev/videoX",
        "usb:046d",
        "usb:zzzz:085e",
        "pipewiresrc fd=7",
        "pipewiresrcfoo",
        "pipewiresrc target-object=",
    ] {
        assert_eq!(
            resolve_privileged_node(source, true),
            None,
            "{source:?} must not resolve for privileged capture"
        );
    }
}

#[test]
fn only_unplugged_v4l2_cameras_count_as_disconnected() {
    assert!(camera_disconnected("/dev/video9999", false));
    assert!(camera_disconnected(
        "pipewiresrc target-object=v4l2_input.pci-0000_ff_1f.7-usb-0_9.9.9_1.0.3",
        false
    ));
    for source in [
        "videotestsrc num-buffers=2",
        "pipewiresrc target-object=alsa_input.pci-0000_00_1f.3",
        "pipewiresrc target-object=51",
        "/dev/videofoo",
        "",
    ] {
        assert!(!camera_disconnected(source, false), "{source:?}");
    }
}

#[test]
fn privileged_open_refuses_custom_pipelines_without_touching_hardware() {
    let err = Camera::open_privileged("videotestsrc num-buffers=2")
        .err()
        .expect("privileged open of a custom pipeline must fail");
    assert!(
        err.to_string().contains("refusing privileged capture"),
        "unexpected error: {err:#}"
    );
    assert!(Camera::open_privileged("").is_err());
    assert!(Camera::open_ir_privileged("v4l2src device=/dev/video0", None).is_err());
}

#[test]
fn libcamera_fallback_only_covers_primary_color_capture() {
    assert!(libcamera_fallback_applies("primary", true, None));
    assert!(libcamera_fallback_applies("  primary ", true, None));
    assert!(!libcamera_fallback_applies("primary", false, None));
    for source in [
        "/dev/video0",
        "usb:046d:085e",
        "pipewiresrc",
        "pipewiresrc target-object=device-name",
        "videotestsrc",
        "libcamerasrc",
    ] {
        assert!(
            !libcamera_fallback_applies(source, true, None),
            "{source:?} must keep its own failure"
        );
    }
}

#[test]
fn libcamera_negotiation_failures_point_at_the_software_isp() {
    let hint = libcamera_negotiation_hint(
        "Internal data stream error.: streaming stopped, reason not-negotiated (-4)",
    )
    .expect("not-negotiated gets a hint");
    assert!(hint.contains("Failed to fork"));
    assert!(libcamera_negotiation_hint("Failed to configure camera: Invalid argument").is_none());
}

#[test]
fn libcamera_fallback_skips_nodes_that_are_not_media_controller_only() {
    assert!(!libcamera_fallback_applies(
        "primary",
        true,
        Some("/dev/video-gaze-test-missing")
    ));
}

#[test]
fn the_querycap_code_matches_the_kernels_ior_encoding() {
    let expected = (2 << 30)
        | ((std::mem::size_of::<V4l2Capability>() as libc::c_ulong) << 16)
        | ((b'V' as libc::c_ulong) << 8);
    assert_eq!(std::mem::size_of::<V4l2Capability>(), 104);
    assert_eq!(VIDIOC_QUERYCAP, expected);
}

#[test]
fn device_caps_are_used_only_when_the_driver_reports_them() {
    let mc_only = V4L2_CAP_IO_MC | 0x1;
    assert_eq!(
        effective_device_caps(V4L2_CAP_DEVICE_CAPS | 0x1, mc_only),
        mc_only
    );
    assert_eq!(effective_device_caps(0x1, mc_only), 0x1);
}

#[test]
fn libcamera_camera_names_survive_pipeline_parsing() {
    assert_eq!(
        libcamera_source_element("\\_SB_.LNK1"),
        "libcamerasrc camera-name=\"\\\\_SB_.LNK1\""
    );
    assert_eq!(
        libcamera_source_element("a \"b\""),
        "libcamerasrc camera-name=\"a \\\"b\\\"\""
    );
}

fn sample_options() -> Vec<(String, String)> {
    vec![
        (IR_NONE_DISPLAY_NAME.to_string(), String::new()),
        ("Integrated IR".to_string(), "/dev/video2".to_string()),
    ]
}

#[test]
fn source_index_resolves_configured_targets() {
    let options = sample_options();
    assert_eq!(source_index(&options, ""), 0);
    assert_eq!(source_index(&options, "/dev/video2"), 1);
}

#[test]
fn source_index_falls_back_for_unlisted_targets() {
    let options = sample_options();
    assert!(!is_listed_source(&options, "/dev/video99"));
    assert_eq!(source_index(&options, "/dev/video99"), 0);
}

#[test]
fn ir_frame_size_constrains_raw_and_jpeg_caps() {
    let pipeline = camera_pipeline("v4l2src device=/dev/video2", false, Some((340, 340)));
    assert!(pipeline.contains("video/x-raw,width=340,height=340,pixel-aspect-ratio=1/1;"));
    assert!(pipeline.contains("image/jpeg,width=340,height=340 ! decodebin"));

    let forced = camera_pipeline("v4l2src device=/dev/video2", true, Some((340, 340)));
    assert!(forced.contains("video/x-raw,format=YUY2,width=340,height=340,"));
}

#[test]
fn ir_frame_size_is_negotiated_when_the_source_supports_it() {
    let mut camera =
        Camera::open_ir("videotestsrc num-buffers=3", Some((480, 480))).expect("ir pipeline");
    let frame = camera.next().expect("videotestsrc frame");
    assert_eq!((frame.cols(), frame.rows()), (480, 480));
}

#[test]
fn unsupported_ir_frame_size_falls_back_to_auto_negotiation() {
    let mut camera = Camera::open_ir(
        "videotestsrc num-buffers=3 ! capsfilter caps=video/x-raw,width=1280,height=720",
        Some((480, 480)),
    )
    .expect("fallback pipeline");
    let frame = camera.next().expect("videotestsrc frame");
    assert_eq!(frame.rows(), 480);
    let cols = frame.cols();
    assert!(
        (853..=854).contains(&cols),
        "expected the source's 16:9 aspect, got {cols}"
    );
}
