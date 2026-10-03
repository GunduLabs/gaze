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
