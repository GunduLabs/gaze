// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::Path;
use std::process::Command;

const ENFORCE_PATH: &str = "/sys/fs/selinux/enforce";
const POLICY_DIR: &str = "/usr/share/gaze";
pub const GDM_CAMERA_MODULE: &str = "gaze-gdm-camera";
pub const GREETER_KEYRING_MODULE: &str = "gaze-greeter-keyring";

pub fn is_enforcing() -> bool {
    std::fs::read_to_string(ENFORCE_PATH).is_ok_and(|value| value.trim() == "1")
}

pub fn policy_path(module: &str) -> String {
    format!("{POLICY_DIR}/{module}.pp")
}

pub enum ModuleState {
    Loaded,
    NotLoaded,
    NeedsRoot,
    Unverifiable(String),
}

pub fn module_state(module: &str) -> ModuleState {
    if unsafe { libc::geteuid() } != 0 {
        return ModuleState::NeedsRoot;
    }
    match Command::new("semodule").arg("-l").output() {
        Ok(output) if output.status.success() => {
            if lists_module(&String::from_utf8_lossy(&output.stdout), module) {
                ModuleState::Loaded
            } else {
                ModuleState::NotLoaded
            }
        }
        Ok(output) => {
            let text = if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            };
            ModuleState::Unverifiable(String::from_utf8_lossy(text).trim().to_string())
        }
        Err(err) => ModuleState::Unverifiable(err.to_string()),
    }
}

pub fn lists_module(output: &str, module: &str) -> bool {
    output
        .lines()
        .any(|line| line.split_whitespace().next() == Some(module))
}

pub fn load_module(module: &str) -> anyhow::Result<()> {
    let path = policy_path(module);
    anyhow::ensure!(
        Path::new(&path).exists(),
        "{path} is missing; reinstall the Gaze package"
    );
    let output = Command::new("semodule").arg("-i").arg(&path).output()?;
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_selinux_module_is_matched_on_the_name_column() {
        let listing = "gaze-gdm-camera\t1.0\nzoneminder\t1.0\n";
        assert!(lists_module(listing, GDM_CAMERA_MODULE));
        assert!(!lists_module("zoneminder\t1.0\n", GDM_CAMERA_MODULE));
        assert!(
            !lists_module("gaze-gdm-camera-extra\t1.0\n", GDM_CAMERA_MODULE),
            "a longer module name sharing the prefix must not match"
        );
        assert!(
            !lists_module("something gaze-gdm-camera\n", GDM_CAMERA_MODULE),
            "only the first column names the module"
        );
    }
}
