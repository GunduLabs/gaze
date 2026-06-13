use crate::ir::devices::{
    CameraBus, IrControl, IrDevice, IrQuery, camera_bus, find_device, usb_ids_of,
};
use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::sync::{Mutex, OnceLock};

const UVC_CTRL_QUERY: libc::c_ulong = 0xC010_7521;
const SET_CUR: u8 = 0x01;
const GET_CUR: u8 = 0x81;

const FACE_AUTH_SELECTOR: u8 = 0x06;
const FACE_AUTH_LEN: usize = 9;
const FACE_AUTH_ON_ALT_FRAME: [u8; FACE_AUTH_LEN] = [1, 3, 2, 0, 0, 0, 0, 0, 0];
const FACE_AUTH_OFF_DISABLED: [u8; FACE_AUTH_LEN] = [1, 3, 1, 0, 0, 0, 0, 0, 0];
const FACE_AUTH_PROBE_MAX_UNIT: u8 = 31;

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
    profile: IrProfile,
}

impl IrLed {
    pub fn for_path(node: &str) -> Option<Self> {
        let (vid, pid) = usb_ids_of(node)?;

        let profile = if let Some(dev) = find_device(vid, pid) {
            IrProfile::from_static(dev)
        } else {
            cached_face_auth_profile(node, vid, pid)?
        };

        Some(Self {
            node: node.to_string(),
            profile,
        })
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    pub fn device_name(&self) -> &str {
        &self.profile.name
    }

    pub fn set(&self, on: bool) -> anyhow::Result<()> {
        let sequence = if on {
            &self.profile.on_sequence
        } else {
            &self.profile.off_sequence
        };

        if sequence.is_empty() {
            return Ok(());
        }

        self.write_sequence(sequence)
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

    if let Some(cached) = cache.lock().unwrap().get(node) {
        return cached.clone();
    }

    if camera_bus(node) == CameraBus::Ipu6 {
        cache.lock().unwrap().insert(node.to_string(), None);
        return None;
    }

    match probe_face_auth_profile(node, vid, pid) {
        Ok(result) => {
            cache
                .lock()
                .unwrap()
                .insert(node.to_string(), result.clone());
            result
        }
        Err(_) => None,
    }
}

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
    };

    #[test]
    fn led_keeps_profile_metadata() {
        let led = IrLed {
            node: "/dev/null".to_string(),
            profile: IrProfile::from_static(&TEST_DEVICE),
        };
        assert_eq!(led.node(), "/dev/null");
        assert_eq!(led.device_name(), "Sample IR Camera");
        assert_eq!(led.profile.source, "unit test");
        assert_eq!(led.profile.on_sequence[0].payload, &[1, 2, 3, 4]);
        assert_eq!(led.profile.off_sequence[0].payload, &[0, 0, 0, 0]);
    }

    #[test]
    fn detects_face_auth_payload_shape() {
        assert!(looks_like_face_auth_control(&FACE_AUTH_OFF_DISABLED));
        assert!(looks_like_face_auth_control(&FACE_AUTH_ON_ALT_FRAME));
        assert!(looks_like_face_auth_control(&[1, 3, 3, 0, 0, 0, 0, 0, 0]));
        assert!(!looks_like_face_auth_control(&[1, 3, 4, 0, 0, 0, 0, 0, 0]));
        assert!(!looks_like_face_auth_control(&[1, 3, 2, 0, 0, 0, 0, 0, 1]));
    }
}
