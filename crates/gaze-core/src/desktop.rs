// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

pub const GDM_DCONF_PROFILE: &str = "gdm";
pub const GDM_DCONF_PROFILE_PATH: &str = "/etc/dconf/profile/gdm";
pub const GDM_DCONF_FACE_AUTH_KEY: &str =
    "/org/gnome/shell/extensions/gaze/enable-face-authentication";
pub const GDM_FACE_OVERRIDE_PATH: &str = "/etc/dconf/db/gdm.d/99-gaze";

pub const KDE_FACE_PAM_FILE: &str = "/etc/pam.d/kde-fingerprint";
pub const KDE_SMARTCARD_PAM_FILE: &str = "/etc/pam.d/kde-smartcard";
pub const PLASMALOGIN_FACE_PAM_FILE: &str = "/etc/pam.d/plasmalogin-fingerprint";

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
    fn gdm_dconf_key_lives_under_the_gaze_extension() {
        assert!(GDM_DCONF_FACE_AUTH_KEY.contains("gaze"));
        assert!(GDM_DCONF_FACE_AUTH_KEY.starts_with("/org/gnome/shell/extensions/"));
        assert_eq!(GDM_DCONF_PROFILE, "gdm");
    }
}
