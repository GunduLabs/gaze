// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

pub const GDM_DCONF_PROFILE: &str = "gdm";
pub const GDM_DCONF_PROFILE_PATH: &str = "/etc/dconf/profile/gdm";
pub const GDM_DCONF_FACE_AUTH_KEY: &str =
    "/org/gnome/shell/extensions/gaze/enable-face-authentication";
pub const GDM_FACE_OVERRIDE_PATH: &str = "/etc/dconf/db/gdm.d/99-gaze";
pub const GDM_HOME_DIRS: [&str; 2] = ["/var/lib/gdm", "/var/lib/gdm3"];

pub const KDE_FACE_PAM_FILE: &str = "/etc/pam.d/kde-fingerprint";
pub const KDE_SMARTCARD_PAM_FILE: &str = "/etc/pam.d/kde-smartcard";
pub const PLASMALOGIN_FACE_PAM_FILE: &str = "/etc/pam.d/plasmalogin-fingerprint";

/// `None` only when `dconf` itself could not answer. An unset key reads back as an empty
/// string, which callers layering one db over another must tell apart from a real value.
pub fn dconf_read_with_profile(
    tag: &str,
    profile_body: &str,
    config_home: Option<&Path>,
    key: &str,
) -> Option<String> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let profile = std::env::temp_dir().join(format!(
        "gaze-{tag}-{}-{}.profile",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::write(&profile, profile_body).ok()?;
    let mut command = Command::new("dconf");
    command.arg("read").arg(key).env("DCONF_PROFILE", &profile);
    if let Some(home) = config_home {
        command.env("XDG_CONFIG_HOME", home);
    }
    let output = command.output();
    let _ = fs::remove_file(&profile);
    let output = output.ok().filter(|output| output.status.success())?;
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn gdm_system_dconf_read(key: &str) -> Option<String> {
    dconf_read_with_profile(
        GDM_DCONF_PROFILE,
        &format!("system-db:{GDM_DCONF_PROFILE}\n"),
        None,
        key,
    )
}

pub fn gdm_user_db_read(dir: &Path, key: &str) -> Option<String> {
    dconf_read_with_profile("gdm-user", "user-db:user\n", Some(dir), key)
        .filter(|value| !value.is_empty())
}

/// The greeter runs as `gdm` with its own `XDG_CONFIG_HOME`, and the path differs by
/// distribution and by seat, so every candidate holding a db has to be considered.
pub fn gdm_greeter_config_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for home in GDM_HOME_DIRS {
        let home = Path::new(home);
        if !home.is_dir() {
            continue;
        }
        dirs.push(home.join(".config"));
        if let Ok(entries) = fs::read_dir(home) {
            for entry in entries.flatten() {
                dirs.push(entry.path().join("config"));
            }
        }
    }
    dirs.retain(|dir| dir.join("dconf/user").is_file());
    dirs.sort();
    dirs.dedup();
    dirs
}

/// `user-db:user` leads the greeter profile, so whatever GDM has written for itself outranks
/// every `system-db` keyfile. Reading only `system-db:gdm` reports what Gaze installed rather
/// than what the greeter resolves, which is how a disabled extension system passed as ready.
pub fn gdm_greeter_dconf_read(key: &str) -> Option<String> {
    for dir in gdm_greeter_config_dirs() {
        if let Some(value) = gdm_user_db_read(&dir, key) {
            return Some(value);
        }
    }
    gdm_system_dconf_read(key)
}

/// Which greeter db holds `key`, for a fix that has to name the file it must be cleared from.
pub fn gdm_greeter_dconf_source(key: &str) -> Option<PathBuf> {
    gdm_greeter_config_dirs()
        .into_iter()
        .find(|dir| gdm_user_db_read(dir, key).is_some())
        .map(|dir| dir.join("dconf/user"))
}

pub fn dconf_bool(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// The value GDM itself sees, which a NixOS configuration sets without `GDM_FACE_OVERRIDE_PATH`.
pub fn gdm_face_auth_from_dconf() -> Option<bool> {
    if !Path::new(GDM_DCONF_PROFILE_PATH).exists() {
        return None;
    }
    dconf_bool(&gdm_greeter_dconf_read(GDM_DCONF_FACE_AUTH_KEY)?)
}

const KSCREENLOCKER_WORKER: &str = "kscreenlocker_worker";
const LIBEXEC_DIRS: [&str; 5] = [
    "usr/libexec",
    "usr/lib",
    "usr/lib64",
    "usr/lib64/libexec",
    "run/current-system/sw/libexec",
];

pub fn kscreenlocker_starts_smartcard_slot() -> bool {
    !kscreenlocker_worker_under(Path::new("/"))
}

fn kscreenlocker_worker_under(root: &Path) -> bool {
    let fixed = LIBEXEC_DIRS.iter().map(|dir| root.join(dir));
    let multiarch = std::iter::once(root.join("usr/lib"))
        .filter_map(|lib| fs::read_dir(lib).ok())
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains("-linux-"))
        .map(|entry| entry.path().join("libexec"));
    fixed
        .chain(multiarch)
        .any(|dir| dir.join(KSCREENLOCKER_WORKER).exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn desktop_paths_are_absolute_and_distinct() {
        let paths = [
            GDM_DCONF_PROFILE_PATH,
            GDM_FACE_OVERRIDE_PATH,
            KDE_FACE_PAM_FILE,
            KDE_SMARTCARD_PAM_FILE,
            PLASMALOGIN_FACE_PAM_FILE,
        ];
        for path in paths {
            assert!(path.starts_with('/'), "{path} must be absolute");
        }
        let unique: HashSet<_> = paths.into_iter().collect();
        assert_eq!(
            unique.len(),
            paths.len(),
            "each integration point needs its own path"
        );
    }

    #[test]
    fn the_plasma_68_worker_is_found_wherever_a_distribution_installs_it() {
        for dir in [
            "usr/libexec",
            "usr/lib",
            "usr/lib/x86_64-linux-gnu/libexec",
            "usr/lib/aarch64-linux-gnu/libexec",
            "run/current-system/sw/libexec",
        ] {
            let root = tempfile::tempdir().unwrap();
            assert!(!kscreenlocker_worker_under(root.path()), "{dir}");
            fs::create_dir_all(root.path().join(dir)).unwrap();
            fs::write(root.path().join(dir).join(KSCREENLOCKER_WORKER), "").unwrap();
            assert!(kscreenlocker_worker_under(root.path()), "{dir}");
        }
    }

    #[test]
    fn the_greeter_alone_does_not_mean_plasma_68() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("usr/libexec")).unwrap();
        fs::write(root.path().join("usr/libexec/kscreenlocker_greet"), "").unwrap();
        assert!(!kscreenlocker_worker_under(root.path()));
    }

    #[test]
    fn gdm_dconf_key_lives_under_the_gaze_extension() {
        assert!(GDM_DCONF_FACE_AUTH_KEY.contains("gaze"));
        assert!(GDM_DCONF_FACE_AUTH_KEY.starts_with("/org/gnome/shell/extensions/"));
        assert_eq!(GDM_DCONF_PROFILE, "gdm");
    }
}
