// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::os::unix::fs::FileTypeExt;
use std::os::unix::io::AsRawFd;
use std::path::Path;

const I2C_SLAVE: libc::c_ulong = 0x0703;
const I2C_RDWR: libc::c_ulong = 0x0707;
const I2C_M_RD: u16 = 0x0001;

#[repr(C)]
struct I2cMsg {
    addr: u16,
    flags: u16,
    len: u16,
    buf: *mut u8,
}

#[repr(C)]
struct I2cRdwrIoctlData {
    msgs: *mut I2cMsg,
    nmsgs: u32,
}

pub struct I2cIrProfile {
    pub name: &'static str,
    pub source: &'static str,
    pub capture_node: &'static str,
    pub capture_name: &'static str,
    pub source_marker: Option<&'static str>,
    pub source_driver: Option<&'static str>,
    pub sensor_device: &'static str,
    pub sensor_driver: &'static str,
    pub address: u16,
    pub register: u16,
    pub register_width: u8,
    pub mask: u8,
    pub on: u8,
    pub off: u8,
}

include!(concat!(env!("OUT_DIR"), "/i2c_ir_profiles.rs"));

pub struct I2cEmitter {
    profile: &'static I2cIrProfile,
    bus: String,
}

impl I2cEmitter {
    pub fn for_path(node: &str) -> Option<Self> {
        match Self::diagnose(node)? {
            Ok(emitter) => Some(emitter),
            Err(reason) => {
                tracing::warn!("I2C IR emitter profile for {node} does not apply: {reason}");
                None
            }
        }
    }

    pub fn diagnose(node: &str) -> Option<Result<Self, String>> {
        let profile = I2C_IR_PROFILES
            .iter()
            .find(|profile| profile.capture_node == node)?;
        Some(resolve_bus(profile).map(|bus| Self { profile, bus }))
    }

    pub fn name(&self) -> &'static str {
        self.profile.name
    }

    pub fn bus(&self) -> &str {
        &self.bus
    }

    pub fn set(&self, on: bool) -> anyhow::Result<()> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.bus)
            .map_err(|e| anyhow::anyhow!("open I2C emitter bus {}: {e}", self.bus))?;
        let fd = file.as_raw_fd();

        self.ensure_address_claimed(fd)?;

        let profile = self.profile;
        let wanted = if on { profile.on } else { profile.off };
        let current = self.read_register(fd)?;
        let next = (current & !profile.mask) | wanted;
        if next != current {
            self.write_register(fd, next)?;
        }

        let readback = self.read_register(fd)?;
        if readback & profile.mask != wanted {
            anyhow::bail!(
                "I2C emitter register 0x{:04x} at 0x{:02x} on {} reads 0x{readback:02x} after writing 0x{next:02x}",
                profile.register,
                profile.address,
                self.bus
            );
        }
        Ok(())
    }

    fn ensure_address_claimed(&self, fd: i32) -> anyhow::Result<()> {
        let address = self.profile.address;
        let result = unsafe { libc::ioctl(fd, I2C_SLAVE, address as libc::c_ulong) };
        if result == 0 {
            anyhow::bail!(
                "no driver claims address 0x{address:02x} on {}; refusing to write to an unidentified device",
                self.bus
            );
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::EBUSY) {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "check I2C address 0x{address:02x} on {}: {err}",
                self.bus
            ))
        }
    }

    fn register_bytes(&self) -> Vec<u8> {
        let [high, low] = self.profile.register.to_be_bytes();
        if self.profile.register_width == 2 {
            vec![high, low]
        } else {
            vec![low]
        }
    }

    fn read_register(&self, fd: i32) -> anyhow::Result<u8> {
        let mut register = self.register_bytes();
        let mut value = [0_u8; 1];
        let mut msgs = [
            I2cMsg {
                addr: self.profile.address,
                flags: 0,
                len: register.len() as u16,
                buf: register.as_mut_ptr(),
            },
            I2cMsg {
                addr: self.profile.address,
                flags: I2C_M_RD,
                len: 1,
                buf: value.as_mut_ptr(),
            },
        ];
        self.transfer(fd, &mut msgs).map_err(|e| {
            anyhow::anyhow!(
                "read I2C emitter register 0x{:04x} at 0x{:02x} on {}: {e}",
                self.profile.register,
                self.profile.address,
                self.bus
            )
        })?;
        Ok(value[0])
    }

    fn write_register(&self, fd: i32, value: u8) -> anyhow::Result<()> {
        let mut bytes = self.register_bytes();
        bytes.push(value);
        let mut msgs = [I2cMsg {
            addr: self.profile.address,
            flags: 0,
            len: bytes.len() as u16,
            buf: bytes.as_mut_ptr(),
        }];
        self.transfer(fd, &mut msgs).map_err(|e| {
            anyhow::anyhow!(
                "write I2C emitter register 0x{:04x} at 0x{:02x} on {}: {e}",
                self.profile.register,
                self.profile.address,
                self.bus
            )
        })
    }

    fn transfer(&self, fd: i32, msgs: &mut [I2cMsg]) -> std::io::Result<()> {
        let mut data = I2cRdwrIoctlData {
            msgs: msgs.as_mut_ptr(),
            nmsgs: msgs.len() as u32,
        };
        let result = unsafe { libc::ioctl(fd, I2C_RDWR, &mut data as *mut I2cRdwrIoctlData) };
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else if result as usize != msgs.len() {
            Err(std::io::Error::other(format!(
                "completed {result} of {} I2C messages",
                msgs.len()
            )))
        } else {
            Ok(())
        }
    }
}

fn resolve_bus(profile: &I2cIrProfile) -> Result<String, String> {
    let node = profile.capture_node;
    if !is_char_device(node) {
        return Err(format!("{node} is not present"));
    }

    let name_path = video_sysfs_path(node, "name");
    let capture_name =
        std::fs::read_to_string(&name_path).map_err(|e| format!("cannot read {name_path}: {e}"))?;
    if capture_name.trim() != profile.capture_name {
        return Err(format!(
            "{name_path} is {:?}, expected {:?}",
            capture_name.trim(),
            profile.capture_name
        ));
    }

    if let (Some(marker), Some(driver)) = (profile.source_marker, profile.source_driver) {
        let source = std::fs::read_to_string(marker).map_err(|e| {
            format!("cannot read bridge marker {marker}: {e}; is the IR bridge running?")
        })?;
        let source = source.trim();
        if !source.starts_with("/dev/video") || !is_char_device(source) {
            return Err(format!(
                "bridge marker {marker} names {source:?}, which is not a video device"
            ));
        }
        let actual = video_driver(source);
        if actual.as_deref() != Some(driver) {
            return Err(format!(
                "bridge source {source} is bound to {}, expected {driver}",
                actual.as_deref().unwrap_or("no driver")
            ));
        }
    }

    let sensor = Path::new("/sys/bus/i2c/devices").join(profile.sensor_device);
    let sensor_driver = std::fs::read_link(sensor.join("driver"))
        .ok()
        .and_then(|link| Some(link.file_name()?.to_str()?.to_string()));
    if sensor_driver.as_deref() != Some(profile.sensor_driver) {
        return Err(format!(
            "I2C device {} is bound to {}, expected {}",
            profile.sensor_device,
            sensor_driver.as_deref().unwrap_or("no driver"),
            profile.sensor_driver
        ));
    }

    let canonical = std::fs::canonicalize(&sensor)
        .map_err(|e| format!("cannot resolve {}: {e}", sensor.display()))?;
    let adapter = canonical
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .and_then(adapter_number)
        .ok_or_else(|| {
            format!(
                "cannot determine the I2C adapter of {}",
                canonical.display()
            )
        })?;

    let bus = format!("/dev/i2c-{adapter}");
    if !is_char_device(&bus) {
        return Err(format!(
            "{bus} is missing; load the i2c-dev kernel module (modprobe i2c-dev)"
        ));
    }
    Ok(bus)
}

fn adapter_number(name: &str) -> Option<u32> {
    name.strip_prefix("i2c-")?.parse().ok()
}

fn is_char_device(path: &str) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_char_device())
}

fn video_sysfs_path(node: &str, attribute: &str) -> String {
    let name = Path::new(node)
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or_default();
    format!("/sys/class/video4linux/{name}/{attribute}")
}

fn video_driver(node: &str) -> Option<String> {
    let link = std::fs::read_link(video_sysfs_path(node, "device/driver")).ok()?;
    Some(link.file_name()?.to_str()?.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface_pro4() -> &'static I2cIrProfile {
        I2C_IR_PROFILES
            .iter()
            .find(|profile| profile.name == "Surface Pro 4 OV7251 IR emitter (I2C)")
            .expect("Surface Pro 4 I2C profile")
    }

    #[test]
    fn generated_profile_table_contains_the_surface_pro4_device() {
        let profile = surface_pro4();
        assert_eq!(profile.sensor_device, "i2c-INT347E:00");
        assert_eq!(profile.sensor_driver, "ov7251");
        assert_eq!(profile.address, 0x60);
        assert_eq!(profile.register, 0x3005);
        assert_eq!(profile.register_width, 2);
        assert_eq!(profile.mask, 0x08);
        assert_eq!(profile.on, 0x08);
        assert_eq!(profile.off, 0x00);
        assert!(profile.source.contains("verified on Surface Pro 4"));
    }

    #[test]
    fn only_the_profile_capture_node_is_considered() {
        assert!(I2cEmitter::diagnose("/dev/video2").is_none());
        assert!(I2cEmitter::diagnose(surface_pro4().capture_node).is_some());
    }

    #[test]
    fn two_byte_registers_are_sent_big_endian() {
        let emitter = I2cEmitter {
            profile: surface_pro4(),
            bus: "/dev/null".to_string(),
        };
        assert_eq!(emitter.register_bytes(), vec![0x30, 0x05]);
    }

    #[test]
    fn adapter_numbers_come_from_the_parent_directory_name() {
        assert_eq!(adapter_number("i2c-3"), Some(3));
        assert_eq!(adapter_number("i2c-12"), Some(12));
        assert_eq!(adapter_number("i2c-INT347E:00"), None);
        assert_eq!(adapter_number("0000:00:15.0"), None);
    }

    #[test]
    fn message_structs_match_the_kernel_layout() {
        assert_eq!(std::mem::size_of::<I2cMsg>(), 16);
        assert_eq!(std::mem::size_of::<I2cRdwrIoctlData>(), 16);
    }
}
