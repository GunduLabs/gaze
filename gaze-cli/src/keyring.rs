// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gaze_core::config::Config;
use gaze_security::keyring::{Zeroizing, validate_password};

pub fn enroll(username: &str, config: &Config) -> anyhow::Result<()> {
    anyhow::ensure!(
        config.storage.unlock_gnome_keyring,
        "enable GNOME Keyring unlock with gaze config first"
    );
    config.storage.validate_keyring(&config.liveness)?;
    // The interactive CLI owns this process; do not change dump policy in a PAM host.
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    anyhow::ensure!(
        unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } == 0,
        "cannot disable credential core dumps"
    );
    gaze_security::keyring::Account::lookup(username)?;
    println!("Enter the login keyring password for {username}.");
    let password = Zeroizing::new(
        dialoguer::Password::new()
            .with_prompt("Login keyring password")
            .with_confirmation("Confirm keyring password", "Passwords did not match")
            .interact()?,
    );
    validate_password(password.as_bytes())?;
    gaze_security::keyring::enroll(username, password.as_bytes())?;
    println!("GNOME Keyring unlock enrolled for {username}.");
    Ok(())
}
