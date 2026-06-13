use std::fs;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IrQuery {
    SetCur,
    GetCur,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrControl {
    pub unit: u8,
    pub selector: u8,
    pub query: IrQuery,
    pub payload: &'static [u8],
}

#[derive(Debug)]
pub struct IrDevice {
    pub vid: u16,
    pub pid: u16,
    pub name: &'static str,
    pub on_sequence: &'static [IrControl],
    pub off_sequence: &'static [IrControl],
    pub source: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/ir_devices.rs"));

pub fn find_device(vid: u16, pid: u16) -> Option<&'static IrDevice> {
    find_in(IR_DEVICES, vid, pid)
}

fn find_in(devices: &[IrDevice], vid: u16, pid: u16) -> Option<&IrDevice> {
    devices.iter().find(|d| d.vid == vid && d.pid == pid)
}

#[derive(Debug, PartialEq, Eq)]
pub enum CameraBus {
    Uvc,
    Ipu6,
    Other,
}

pub fn usb_ids_of(node: &str) -> Option<(u16, u16)> {
    parse_modalias(&read_modalias(node)?)
}

pub fn camera_bus(node: &str) -> CameraBus {
    match read_driver_basename(node) {
        Some(driver) => bus_of_driver(&driver),
        None => CameraBus::Other,
    }
}

fn sysfs_device_dir(node: &str) -> Option<String> {
    let name = Path::new(node).file_name()?.to_str()?;
    Some(format!("/sys/class/video4linux/{name}/device"))
}

fn read_modalias(node: &str) -> Option<String> {
    fs::read_to_string(format!("{}/modalias", sysfs_device_dir(node)?)).ok()
}

fn read_driver_basename(node: &str) -> Option<String> {
    let link = fs::read_link(format!("{}/driver", sysfs_device_dir(node)?)).ok()?;
    Some(link.file_name()?.to_str()?.to_string())
}

fn parse_modalias(modalias: &str) -> Option<(u16, u16)> {
    let s = modalias.trim().strip_prefix("usb:")?.strip_prefix('v')?;
    let vid = u16::from_str_radix(s.get(..4)?, 16).ok()?;
    let rest = s.get(4..)?.strip_prefix('p')?;
    let pid = u16::from_str_radix(rest.get(..4)?, 16).ok()?;
    Some((vid, pid))
}

fn bus_of_driver(driver: &str) -> CameraBus {
    if driver.contains("ipu6") || driver.contains("intel_ipu") {
        CameraBus::Ipu6
    } else if driver == "uvcvideo" {
        CameraBus::Uvc
    } else {
        CameraBus::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_ON_BYTES: &[u8] = &[1, 2, 3, 4];
    const SAMPLE_OFF_BYTES: &[u8] = &[0, 0, 0, 0];
    const SAMPLE_ON: &[IrControl] = &[IrControl {
        unit: 3,
        selector: 2,
        query: IrQuery::SetCur,
        payload: SAMPLE_ON_BYTES,
    }];
    const SAMPLE_OFF: &[IrControl] = &[IrControl {
        unit: 3,
        selector: 2,
        query: IrQuery::SetCur,
        payload: SAMPLE_OFF_BYTES,
    }];
    const SAMPLE: &[IrDevice] = &[IrDevice {
        vid: 0x1234,
        pid: 0x5678,
        name: "Sample IR Camera",
        on_sequence: SAMPLE_ON,
        off_sequence: SAMPLE_OFF,
        source: "unit test",
    }];

    #[test]
    fn generated_table_contains_researched_profiles() {
        assert!(IR_DEVICES.len() >= 7);
        assert!(find_device(0x04f2, 0xb67c).is_some());
        assert!(find_device(0x04f2, 0xb6d9).is_some());
        assert!(find_device(0x0bda, 0x5767).is_some());
        assert!(find_device(0x30c9, 0x0057).is_some());
    }

    #[test]
    fn known_profiles_have_off_sequences() {
        for device in IR_DEVICES {
            assert!(!device.name.is_empty());
            assert!(!device.source.is_empty());
            assert!(
                !device.on_sequence.is_empty(),
                "{} missing on sequence",
                device.name
            );
            assert!(
                !device.off_sequence.is_empty(),
                "{} missing off sequence",
                device.name
            );
        }
    }

    #[test]
    fn find_in_matches_a_known_vid_pid() {
        let d = find_in(SAMPLE, 0x1234, 0x5678).expect("sample present");
        assert_eq!(d.on_sequence[0].unit, 3);
        assert_eq!(d.on_sequence[0].selector, 2);
        assert_eq!(d.on_sequence[0].payload, &[1, 2, 3, 4]);
    }

    #[test]
    fn find_in_rejects_unknown_vid_pid() {
        assert!(find_in(SAMPLE, 0xDEAD, 0xBEEF).is_none());
    }

    #[test]
    fn find_device_rejects_unknown_vid_pid() {
        assert!(find_device(0x1234, 0x5678).is_none());
    }

    #[test]
    fn modalias_yields_vid_and_pid() {
        assert_eq!(
            parse_modalias("usb:v1A2BpC3D4d0100dc00"),
            Some((0x1A2B, 0xC3D4))
        );
        assert_eq!(
            parse_modalias("usb:v0BDApF00Dd0001"),
            Some((0x0BDA, 0xF00D))
        );
    }

    #[test]
    fn modalias_rejects_malformed() {
        assert_eq!(parse_modalias("pci:v00008086"), None);
        assert_eq!(parse_modalias(""), None);
        assert_eq!(parse_modalias("usb:vZZZZpC3D4"), None);
    }

    #[test]
    fn driver_name_classifies_bus() {
        assert_eq!(bus_of_driver("intel_ipu6_isys"), CameraBus::Ipu6);
        assert_eq!(bus_of_driver("ipu6"), CameraBus::Ipu6);
        assert_eq!(bus_of_driver("uvcvideo"), CameraBus::Uvc);
        assert_eq!(bus_of_driver("something_else"), CameraBus::Other);
    }
}
