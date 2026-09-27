// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::Path;

const I2C_SLAVE_FORCE: libc::c_ulong = 0x0706;

pub struct I2cIrProfile {
    pub name: &'static str,
    pub source: &'static str,
    pub capture_node: &'static str,
    pub capture_name: &'static str,
    pub source_marker: Option<&'static str>,
    pub source_driver: Option<&'static str>,
    pub sensor_uevent: Option<&'static str>,
    pub sensor_driver: Option<&'static str>,
    pub bus: &'static str,
    pub address: u16,
    pub on: &'static [&'static [u8]],
    pub off: &'static [&'static [u8]],
}

include!(concat!(env!("OUT_DIR"), "/i2c_ir_profiles.rs"));

pub struct I2cEmitter {
    profile: &'static I2cIrProfile,
}

impl I2cEmitter {
    pub fn for_path(node: &str) -> Option<Self> {
        I2C_IR_PROFILES
            .iter()
            .find(|profile| profile_matches_path(profile, node))
            .map(|profile| Self { profile })
    }

    pub fn name(&self) -> &'static str {
        self.profile.name
    }

    pub fn source(&self) -> &'static str {
        self.profile.source
    }

    pub fn set(&self, on: bool) -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.profile.bus)
            .map_err(|e| anyhow::anyhow!("open I2C emitter bus {}: {e}", self.profile.bus))?;

        let result = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                I2C_SLAVE_FORCE,
                self.profile.address as libc::c_ulong,
            )
        };
        if result < 0 {
            return Err(anyhow::anyhow!(
                "select I2C emitter address 0x{:02x} on {}: {}",
                self.profile.address,
                self.profile.bus,
                std::io::Error::last_os_error()
            ));
        }

        let writes = if on {
            self.profile.on
        } else {
            self.profile.off
        };
        for bytes in writes {
            let written = file.write(bytes).map_err(|e| {
                anyhow::anyhow!(
                    "write I2C emitter sequence to {} at 0x{:02x}: {e}",
                    self.profile.bus,
                    self.profile.address
                )
            })?;
            if written != bytes.len() {
                return Err(anyhow::anyhow!(
                    "short I2C emitter write to {} at 0x{:02x}: wrote {written} of {} bytes",
                    self.profile.bus,
                    self.profile.address,
                    bytes.len()
                ));
            }
        }
        Ok(())
    }
}

fn profile_matches_path(profile: &I2cIrProfile, node: &str) -> bool {
    if node != profile.capture_node || !Path::new(node).exists() {
        return false;
    }

    let name_path = video_sysfs_path(node, "name");
    let Ok(capture_name) = std::fs::read_to_string(name_path) else {
        return false;
    };
    if capture_name.trim() != profile.capture_name {
        return false;
    }

    if let Some(marker) = profile.source_marker {
        let Ok(source) = std::fs::read_to_string(marker) else {
            return false;
        };
        let source = source.trim();
        if source.is_empty() || !Path::new(source).exists() {
            return false;
        }
        if let Some(driver) = profile.source_driver
            && video_driver(source).as_deref() != Some(driver)
        {
            return false;
        }
    }

    if let (Some(uevent), Some(driver)) = (profile.sensor_uevent, profile.sensor_driver) {
        let Ok(uevent) = std::fs::read_to_string(uevent) else {
            return false;
        };
        if !uevent
            .lines()
            .any(|line| line.trim() == format!("DRIVER={driver}"))
        {
            return false;
        }
    }

    Path::new(profile.bus).exists()
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

    #[test]
    fn generated_profile_table_contains_the_surface_pro4_device() {
        let profile = I2C_IR_PROFILES
            .iter()
            .find(|profile| profile.name == "Surface Pro 4 OV7251 IR emitter (I2C)")
            .expect("Surface Pro 4 I2C profile");
        assert_eq!(profile.bus, "/dev/i2c-3");
        assert_eq!(profile.address, 0x60);
        assert_eq!(profile.on, &[&[0x30, 0x05, 0x08][..]]);
        assert_eq!(profile.off, &[&[0x30, 0x05, 0x00][..]]);
        assert!(profile.source.contains("verified on Surface Pro 4"));
    }

    #[test]
    fn a_profile_does_not_match_another_capture_node() {
        let profile = &I2C_IR_PROFILES[0];
        assert!(!profile_matches_path(profile, "/dev/video2"));
    }
}
