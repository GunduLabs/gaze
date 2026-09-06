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
    println!(
        "Enrolling GNOME Keyring unlock for {username}. Enter the login keyring password, normally your login password."
    );
    println!(
        "This stores a TPM-protected password equivalent. Root can recover it. Re-enroll after changing your account or keyring password."
    );
    let password = Zeroizing::new(
        dialoguer::Password::new()
            .with_prompt("Login keyring password")
            .with_confirmation("Confirm keyring password", "Passwords did not match")
            .interact()?,
    );
    validate_password(password.as_bytes())?;
    gaze_security::keyring::enroll(username, password.as_bytes())?;
    println!("TPM-protected GNOME Keyring credential enrolled for {username}.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{Cli, Commands, command_requires_root};
    use clap::Parser;

    #[test]
    fn keyring_management_requires_root_and_parses_user_and_forget() {
        let cli = Cli::try_parse_from(["gaze", "keyring", "--user", "alice", "--forget"]).unwrap();
        assert_eq!(command_requires_root(&cli.command), Some("keyring"));
        assert!(
            matches!(cli.command, Commands::Keyring { user: Some(user), forget: true } if user == "alice")
        );
        let cli = Cli::try_parse_from(["gaze", "keyring"]).unwrap();
        assert_eq!(command_requires_root(&cli.command), Some("keyring"));
        assert!(matches!(
            cli.command,
            Commands::Keyring {
                user: None,
                forget: false
            }
        ));
    }

    #[test]
    fn passwords_cannot_be_supplied_as_command_line_arguments() {
        assert!(Cli::try_parse_from(["gaze", "keyring", "--password", "secret"]).is_err());
        assert!(Cli::try_parse_from(["gaze", "keyring", "secret"]).is_err());
    }
}
