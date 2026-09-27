// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::ir::devices::{
    CameraBus, IrControl, IrDevice, IrQuery, camera_bus, find_device, usb_ids_of,
};
use std::collections::HashMap;
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

// UVCIOC_CTRL_QUERY, i.e. _IOWR('u', 0x21, struct uvc_xu_control_query). Hardcoded because
// libc exposes no uvcvideo bindings; the 0x0010 in the middle is this struct's 16-byte size.
const UVC_CTRL_QUERY: libc::c_ulong = 0xC010_7521;
const SET_CUR: u8 = 0x01;
const GET_CUR: u8 = 0x81;

// Microsoft's Face Authentication extension unit, shared by most Windows Hello cameras. Byte 2
// of the 9-byte payload is the emitter mode, where 1 is off and 2 strobes on alternate frames.
const FACE_AUTH_SELECTOR: u8 = 0x06;
const FACE_AUTH_LEN: usize = 9;
const FACE_AUTH_ON_ALT_FRAME: [u8; FACE_AUTH_LEN] = [1, 3, 2, 0, 0, 0, 0, 0, 0];
const FACE_AUTH_OFF_DISABLED: [u8; FACE_AUTH_LEN] = [1, 3, 1, 0, 0, 0, 0, 0, 0];
const FACE_AUTH_PROBE_MAX_UNIT: u8 = 31;

// Linux i2c-dev ioctl. The Surface Pro 4's OV7251 is an ACPI/IPU3 sensor, so
// it has no UVC extension-unit node. This backend is enabled only after the
// live IPU3 source and the ACPI OV7251 driver are both identified below.
const I2C_SLAVE_FORCE: libc::c_ulong = 0x0706;
const SURFACE_I2C_BUS: &str = "/dev/i2c-3";
const SURFACE_I2C_ADDRESS: u16 = 0x60;
const SURFACE_STROBE_REGISTER_HIGH: u8 = 0x30;
const SURFACE_STROBE_REGISTER_LOW: u8 = 0x05;
const SURFACE_STROBE_ON: u8 = 0x08;
const SURFACE_STROBE_OFF: u8 = 0x00;
const SURFACE_BRIDGE_NODE: &str = "/dev/video42";
const SURFACE_BRIDGE_NAME: &str = "Surface IR Camera";
const SURFACE_RUNTIME_INPUT: &str = "/run/surface_ir_bridge_dev";
const SURFACE_OV7251_UEVENT: &str = "/sys/bus/i2c/devices/i2c-INT347E:00/uevent";

#[repr(C)]
struct XuCtrlQuery {
    unit: u8,
    selector: u8,
    query: u8,
    _reserved0: u8,
    size: u16,
    _reserved1: u16,
    data: *mut u8,
}

const _: () = assert!(std::mem::size_of::<XuCtrlQuery>() == 16);

#[derive(Clone, Debug, Eq, PartialEq)]
struct RuntimeControl {
    unit: u8,
    selector: u8,
    query: IrQuery,
    payload: Vec<u8>,
}

impl RuntimeControl {
    fn from_static(control: &IrControl) -> Self {
        Self {
            unit: control.unit,
            selector: control.selector,
            query: control.query,
            payload: control.payload.to_vec(),
        }
    }

    fn set(unit: u8, selector: u8, payload: &[u8]) -> Self {
        Self {
            unit,
            selector,
            query: IrQuery::SetCur,
            payload: payload.to_vec(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IrProfile {
    name: String,
    on_sequence: Vec<RuntimeControl>,
    off_sequence: Vec<RuntimeControl>,
    source: String,
}

impl IrProfile {
    fn from_static(device: &IrDevice) -> Self {
        Self {
            name: device.name.to_string(),
            on_sequence: device
                .on_sequence
                .iter()
                .map(RuntimeControl::from_static)
                .collect(),
            off_sequence: device
                .off_sequence
                .iter()
                .map(RuntimeControl::from_static)
                .collect(),
            source: device.source.to_string(),
        }
    }
}

pub struct IrLed {
    node: String,
    backend: IrLedBackend,
}

enum IrLedBackend {
    Uvc(IrProfile),
    SurfaceI2c(SurfaceI2cLed),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SurfaceI2cLed {
    bus: String,
    address: u16,
}

impl IrLed {
    pub fn for_path(node: &str) -> Option<Self> {
        if let Some(surface_i2c) = surface_ov7251_backend(node) {
            return Some(Self {
                node: node.to_string(),
                backend: IrLedBackend::SurfaceI2c(surface_i2c),
            });
        }

        let (vid, pid) = usb_ids_of(node)?;

        let profile = if let Some(dev) = find_device(vid, pid) {
            IrProfile::from_static(dev)
        } else {
            cached_face_auth_profile(node, vid, pid)?
        };

        Some(Self {
            node: node.to_string(),
            backend: IrLedBackend::Uvc(profile),
        })
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    pub fn device_name(&self) -> &str {
        match &self.backend {
            IrLedBackend::Uvc(profile) => &profile.name,
            IrLedBackend::SurfaceI2c(_) => "Surface Pro 4 OV7251 IR emitter (I2C)",
        }
    }

    pub fn set(&self, on: bool) -> anyhow::Result<()> {
        match &self.backend {
            IrLedBackend::SurfaceI2c(backend) => backend.set(on),
            IrLedBackend::Uvc(profile) => {
                let sequence = if on {
                    &profile.on_sequence
                } else {
                    &profile.off_sequence
                };

                if sequence.is_empty() {
                    return Ok(());
                }

                self.write_sequence(sequence)
            }
        }
    }

    fn write_sequence(&self, sequence: &[RuntimeControl]) -> anyhow::Result<()> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.node)?;

        for step in sequence {
            self.write_control(file.as_raw_fd(), step)?;
        }

        Ok(())
    }

    fn write_control(&self, fd: i32, step: &RuntimeControl) -> anyhow::Result<()> {
        let mut payload = step.payload.clone();
        let query_code = match step.query {
            IrQuery::SetCur => SET_CUR,
            IrQuery::GetCur => GET_CUR,
        };

        xu_ioctl(fd, step.unit, step.selector, query_code, &mut payload).map_err(|e| {
            anyhow::anyhow!(
                "UVC {:?} control ioctl on {} failed for unit=0x{:02x} selector=0x{:02x} size={}: {}",
                step.query,
                self.node,
                step.unit,
                step.selector,
                step.payload.len(),
                e
            )
        })
    }
}

impl SurfaceI2cLed {
    fn set(&self, on: bool) -> anyhow::Result<()> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.bus)
            .map_err(|e| anyhow::anyhow!("open Surface IR emitter bus {}: {e}", self.bus))?;

        let result = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                I2C_SLAVE_FORCE,
                self.address as libc::c_ulong,
            )
        };
        if result < 0 {
            return Err(anyhow::anyhow!(
                "select Surface IR emitter address 0x{:02x} on {}: {}",
                self.address,
                self.bus,
                std::io::Error::last_os_error()
            ));
        }

        let value = if on {
            SURFACE_STROBE_ON
        } else {
            SURFACE_STROBE_OFF
        };
        let bytes = [
            SURFACE_STROBE_REGISTER_HIGH,
            SURFACE_STROBE_REGISTER_LOW,
            value,
        ];
        (&file).write_all(&bytes).map_err(|e| {
            anyhow::anyhow!(
                "write Surface OV7251 strobe register 0x3005=0x{value:02x} on {}: {e}",
                self.bus
            )
        })?;
        Ok(())
    }
}

fn surface_ov7251_backend(node: &str) -> Option<SurfaceI2cLed> {
    if !is_surface_bridge_node(
        node,
        &std::fs::read_to_string("/sys/class/video4linux/video42/name").ok()?,
    ) || !Path::new(node).exists()
    {
        return None;
    }

    // The bridge records the current CIO2 source node at boot. Requiring the
    // IPU3 driver here prevents a generic loopback or USB camera from being
    // mistaken for this hardware-specific backend.
    let source = std::fs::read_to_string(SURFACE_RUNTIME_INPUT).ok()?;
    let source = source.trim();
    if source.is_empty() || !Path::new(source).exists() {
        return None;
    }
    let source_name = Path::new(source).file_name()?.to_str()?;
    let driver = std::fs::read_link(format!(
        "/sys/class/video4linux/{source_name}/device/driver"
    ))
    .ok()?;
    if driver.file_name()?.to_str()? != "ipu3-cio2" {
        return None;
    }
    let uevent = std::fs::read_to_string(SURFACE_OV7251_UEVENT).ok()?;
    if !uevent.lines().any(|line| line.trim() == "DRIVER=ov7251") {
        return None;
    }
    if !Path::new(SURFACE_I2C_BUS).exists() {
        return None;
    }

    tracing::debug!(
        node,
        source,
        "detected Surface Pro 4 OV7251 I2C emitter backend"
    );
    Some(SurfaceI2cLed {
        bus: SURFACE_I2C_BUS.to_string(),
        address: SURFACE_I2C_ADDRESS,
    })
}

fn is_surface_bridge_node(node: &str, device_name: &str) -> bool {
    Path::new(node) == Path::new(SURFACE_BRIDGE_NODE) && device_name.trim() == SURFACE_BRIDGE_NAME
}

fn xu_ioctl(
    fd: i32,
    unit: u8,
    selector: u8,
    query_code: u8,
    payload: &mut [u8],
) -> std::io::Result<()> {
    let mut query = XuCtrlQuery {
        unit,
        selector,
        query: query_code,
        _reserved0: 0,
        size: payload.len() as u16,
        _reserved1: 0,
        data: payload.as_mut_ptr(),
    };

    let ret = unsafe { libc::ioctl(fd, UVC_CTRL_QUERY, &mut query as *mut XuCtrlQuery) };
    if ret < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

static PROBE_CACHE: OnceLock<Mutex<HashMap<String, Option<IrProfile>>>> = OnceLock::new();

fn cached_face_auth_profile(node: &str, vid: u16, pid: u16) -> Option<IrProfile> {
    let cache = PROBE_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let lock = || cache.lock().unwrap_or_else(|err| err.into_inner());

    if let Some(cached) = lock().get(node) {
        return cached.clone();
    }

    if camera_bus(node) == CameraBus::Ipu6 {
        lock().insert(node.to_string(), None);
        return None;
    }

    match probe_face_auth_profile(node, vid, pid) {
        Ok(result) => {
            lock().insert(node.to_string(), result.clone());
            result
        }
        Err(_) => None,
    }
}

/// Extension unit ids are assigned per device and V4L2 offers no way to enumerate them, so the
/// only way to find the face-auth unit on an unlisted camera is to GET_CUR every id in turn.
fn probe_face_auth_profile(node: &str, vid: u16, pid: u16) -> anyhow::Result<Option<IrProfile>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)?;
    let fd = file.as_raw_fd();

    for unit in 1..=FACE_AUTH_PROBE_MAX_UNIT {
        let mut cur = [0_u8; FACE_AUTH_LEN];
        if xu_ioctl(fd, unit, FACE_AUTH_SELECTOR, GET_CUR, &mut cur).is_ok()
            && looks_like_face_auth_control(&cur)
        {
            return Ok(Some(IrProfile {
                name: format!(
                    "USB {:04x}:{:04x} Microsoft Face Authentication UVC control",
                    vid, pid
                ),
                on_sequence: vec![RuntimeControl::set(
                    unit,
                    FACE_AUTH_SELECTOR,
                    &FACE_AUTH_ON_ALT_FRAME,
                )],
                off_sequence: vec![RuntimeControl::set(
                    unit,
                    FACE_AUTH_SELECTOR,
                    &FACE_AUTH_OFF_DISABLED,
                )],
                source: "runtime probe: selector 0x06 exposes Microsoft Face Authentication-style [1,3,mode,0...] control".to_string(),
            }));
        }
    }

    Ok(None)
}

fn looks_like_face_auth_control(payload: &[u8; FACE_AUTH_LEN]) -> bool {
    payload[0] == 1
        && payload[1] == 3
        && (1..=3).contains(&payload[2])
        && payload[3..].iter().all(|byte| *byte == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_struct_matches_kernel_layout() {
        assert_eq!(std::mem::size_of::<XuCtrlQuery>(), 16);
    }

    #[test]
    fn for_path_returns_none_when_device_absent() {
        assert!(IrLed::for_path("/dev/video-absent-12345").is_none());
    }

    static TEST_ON_BYTES: &[u8] = &[1, 2, 3, 4];
    static TEST_OFF_BYTES: &[u8] = &[0, 0, 0, 0];
    static TEST_ON: &[IrControl] = &[IrControl {
        unit: 3,
        selector: 2,
        query: IrQuery::SetCur,
        payload: TEST_ON_BYTES,
    }];

    static TEST_OFF: &[IrControl] = &[IrControl {
        unit: 3,
        selector: 2,
        query: IrQuery::SetCur,
        payload: TEST_OFF_BYTES,
    }];

    static TEST_DEVICE: IrDevice = IrDevice {
        vid: 0x1234,
        pid: 0x5678,
        name: "Sample IR Camera",
        on_sequence: TEST_ON,
        off_sequence: TEST_OFF,
        source: "unit test",
        requires_ir_yuy2: false,
    };

    #[test]
    fn led_keeps_profile_metadata() {
        let led = IrLed {
            node: "/dev/null".to_string(),
            backend: IrLedBackend::Uvc(IrProfile::from_static(&TEST_DEVICE)),
        };
        assert_eq!(led.node(), "/dev/null");
        assert_eq!(led.device_name(), "Sample IR Camera");
        let IrLedBackend::Uvc(profile) = led.backend else {
            panic!("expected UVC backend");
        };
        assert_eq!(profile.source, "unit test");
        assert_eq!(profile.on_sequence[0].payload, &[1, 2, 3, 4]);
        assert_eq!(profile.off_sequence[0].payload, &[0, 0, 0, 0]);
    }

    #[test]
    fn detects_face_auth_payload_shape() {
        assert!(looks_like_face_auth_control(&FACE_AUTH_OFF_DISABLED));
        assert!(looks_like_face_auth_control(&FACE_AUTH_ON_ALT_FRAME));
        assert!(looks_like_face_auth_control(&[1, 3, 3, 0, 0, 0, 0, 0, 0]));
        assert!(!looks_like_face_auth_control(&[1, 3, 4, 0, 0, 0, 0, 0, 0]));
        assert!(!looks_like_face_auth_control(&[1, 3, 2, 0, 0, 0, 0, 0, 1]));
    }

    fn led_with(node: &str, on: Vec<RuntimeControl>, off: Vec<RuntimeControl>) -> IrLed {
        IrLed {
            node: node.to_string(),
            backend: IrLedBackend::Uvc(IrProfile {
                name: "Test".to_string(),
                on_sequence: on,
                off_sequence: off,
                source: "unit test".to_string(),
            }),
        }
    }

    #[test]
    fn surface_backend_only_matches_its_named_bridge_output() {
        assert!(is_surface_bridge_node(
            SURFACE_BRIDGE_NODE,
            SURFACE_BRIDGE_NAME
        ));
        assert!(!is_surface_bridge_node("/dev/video2", SURFACE_BRIDGE_NAME));
        assert!(!is_surface_bridge_node(SURFACE_BRIDGE_NODE, "USB Camera"));
    }

    #[test]
    fn the_query_code_matches_the_kernels_iowr_encoding() {
        // _IOWR('u', 0x21, struct uvc_xu_control_query): dir=3, size=16, type='u', nr=0x21.
        let expected = (3 << 30)
            | ((std::mem::size_of::<XuCtrlQuery>() as libc::c_ulong) << 16)
            | ((b'u' as libc::c_ulong) << 8)
            | 0x21;
        assert_eq!(UVC_CTRL_QUERY, expected);
        assert_eq!(UVC_CTRL_QUERY, 0xC010_7521);
    }

    #[test]
    fn the_uvc_query_codes_are_the_ones_the_spec_defines() {
        assert_eq!(SET_CUR, 0x01);
        assert_eq!(GET_CUR, 0x81);
    }

    #[test]
    fn the_face_auth_payloads_differ_only_in_the_emitter_mode() {
        assert_eq!(FACE_AUTH_ON_ALT_FRAME.len(), FACE_AUTH_LEN);
        assert_eq!(FACE_AUTH_OFF_DISABLED.len(), FACE_AUTH_LEN);

        for (index, (on, off)) in FACE_AUTH_ON_ALT_FRAME
            .iter()
            .zip(FACE_AUTH_OFF_DISABLED.iter())
            .enumerate()
        {
            if index == 2 {
                assert_ne!(on, off, "byte 2 carries the emitter mode");
            } else {
                assert_eq!(on, off, "byte {index} is shared by both payloads");
            }
        }
        assert_eq!(
            FACE_AUTH_ON_ALT_FRAME[2], 2,
            "2 strobes on alternate frames"
        );
        assert_eq!(FACE_AUTH_OFF_DISABLED[2], 1, "1 disables the emitter");
    }

    #[test]
    fn a_face_auth_probe_rejects_payloads_from_unrelated_controls() {
        assert!(!looks_like_face_auth_control(&[0, 3, 2, 0, 0, 0, 0, 0, 0]));
        assert!(!looks_like_face_auth_control(&[1, 2, 2, 0, 0, 0, 0, 0, 0]));
        assert!(!looks_like_face_auth_control(&[1, 3, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!looks_like_face_auth_control(&[0; FACE_AUTH_LEN]));
        assert!(!looks_like_face_auth_control(&[0xff; FACE_AUTH_LEN]));
    }

    #[test]
    fn a_face_auth_probe_only_walks_units_the_spec_allows() {
        const { assert!(FACE_AUTH_PROBE_MAX_UNIT >= 1) };
        const {
            assert!(
                FACE_AUTH_PROBE_MAX_UNIT <= 31,
                "UVC entity ids are 5 bits wide"
            )
        };
        assert_eq!(FACE_AUTH_SELECTOR, 0x06);
    }

    #[test]
    fn a_runtime_control_keeps_everything_the_static_control_carried() {
        let converted = RuntimeControl::from_static(&TEST_ON[0]);

        assert_eq!(converted, RuntimeControl::set(3, 2, &[1, 2, 3, 4]));
        assert_eq!(converted.query, IrQuery::SetCur);
        assert_eq!(converted.payload, vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_get_cur_step_does_not_silently_become_a_set_cur() {
        static PROBE_BYTES: &[u8] = &[0; 9];
        static PROBE: IrControl = IrControl {
            unit: 7,
            selector: 6,
            query: IrQuery::GetCur,
            payload: PROBE_BYTES,
        };

        let converted = RuntimeControl::from_static(&PROBE);

        assert_eq!(converted.query, IrQuery::GetCur);
        assert_ne!(converted, RuntimeControl::set(7, 6, PROBE_BYTES));
    }

    #[test]
    fn a_profile_carries_both_sequences_over_from_its_static_device() {
        let profile = IrProfile::from_static(&TEST_DEVICE);

        assert_eq!(profile.name, "Sample IR Camera");
        assert_eq!(profile.source, "unit test");
        assert_eq!(profile.on_sequence.len(), 1);
        assert_eq!(profile.off_sequence.len(), 1);
        assert_eq!(profile.on_sequence[0].unit, 3);
        assert_eq!(profile.on_sequence[0].selector, 2);
    }

    #[test]
    fn an_empty_sequence_short_circuits_before_the_device_is_opened() {
        // A profile with nothing to send must not fail just because its node is gone.
        let led = led_with("/dev/gaze-no-such-node", Vec::new(), Vec::new());

        led.set(true).expect("an empty on-sequence is a no-op");
        led.set(false).expect("an empty off-sequence is a no-op");
    }

    #[test]
    fn a_sequence_against_a_missing_node_reports_the_open_failure() {
        let led = led_with(
            "/dev/gaze-no-such-node",
            vec![RuntimeControl::set(3, 2, &[1])],
            vec![RuntimeControl::set(3, 2, &[0])],
        );

        assert!(led.set(true).is_err());
        assert!(led.set(false).is_err());
    }

    #[test]
    fn an_ioctl_against_a_non_uvc_node_fails_instead_of_reporting_success() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let mut payload = [0_u8; FACE_AUTH_LEN];

        let result = xu_ioctl(
            file.as_raw_fd(),
            1,
            FACE_AUTH_SELECTOR,
            GET_CUR,
            &mut payload,
        );

        assert!(result.is_err(), "/dev/null cannot answer a UVC XU query");
    }

    #[test]
    fn an_ioctl_failure_names_the_node_and_the_control_that_failed() {
        let led = led_with("/dev/null", Vec::new(), Vec::new());
        let file = std::fs::File::open("/dev/null").unwrap();

        let err = led
            .write_control(file.as_raw_fd(), &RuntimeControl::set(0x0c, 0x06, &[0; 9]))
            .expect_err("/dev/null cannot answer a UVC XU query");
        let message = err.to_string();

        assert!(message.contains("/dev/null"), "{message}");
        assert!(message.contains("unit=0x0c"), "{message}");
        assert!(message.contains("selector=0x06"), "{message}");
        assert!(message.contains("size=9"), "{message}");
    }

    #[test]
    fn for_path_ignores_a_path_that_is_not_a_video_node() {
        assert!(IrLed::for_path("/dev/null").is_none());
        assert!(IrLed::for_path("").is_none());
    }
}
