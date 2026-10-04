// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gaze_core::config::Config;
use gaze_security::keyring::{Backend, Zeroizing};

/// The CLI must refuse before touching core dumps or prompting when the
/// backend is switched off in config. Pure so it can be tested without PAM.
fn ensure_backend_enabled(config: &Config, backend: Backend) -> anyhow::Result<()> {
    let enabled = match backend {
        Backend::Gnome => config.storage.unlock_gnome_keyring,
        Backend::KWallet => config.storage.unlock_kwallet,
    };
    anyhow::ensure!(
        enabled,
        "enable {} unlock with gaze config first",
        backend.name()
    );
    Ok(())
}

pub fn enroll(username: &str, config: &Config, backend: Backend) -> anyhow::Result<()> {
    ensure_backend_enabled(config, backend)?;
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
    // Fail before prompting for an unusable account; enrollment checks again for account changes.
    gaze_security::keyring::Account::lookup(username)?;
    println!("Enter the {} password for {username}.", backend.name());
    let password = Zeroizing::new(
        dialoguer::Password::new()
            .with_prompt(format!("{} password", backend.name()))
            .with_confirmation("Confirm keyring password", "Passwords did not match")
            .interact()?,
    );
    gaze_security::keyring::enroll_for(backend, username, password.as_bytes())?;
    println!("{} unlock enrolled for {username}.", backend.name());
    // The greeter's PAM worker is confined as xdm_t, which the distribution policy keeps
    // away from /etc/shadow and the TPM, so the record just written is unusable there.
    if crate::selinux::is_enforcing() {
        let module = crate::selinux::GREETER_KEYRING_MODULE;
        match crate::selinux::load_module(module) {
            Ok(()) => println!("Loaded the {module} SELinux policy for the login screen."),
            Err(err) => eprintln!(
                "Warning: the {module} SELinux policy could not be loaded ({err}). Face login \
                 cannot unlock {} until you run `sudo semodule -i {}`.",
                backend.name(),
                crate::selinux::policy_path(module)
            ),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(unlock_gnome: bool, unlock_kwallet: bool) -> Config {
        let mut config = Config::default();
        config.storage.unlock_gnome_keyring = unlock_gnome;
        config.storage.unlock_kwallet = unlock_kwallet;
        config
    }

    #[test]
    fn disabled_backend_is_rejected_before_any_side_effect() {
        let config = config_with(false, false);
        for backend in [Backend::Gnome, Backend::KWallet] {
            let err = ensure_backend_enabled(&config, backend).expect_err("must refuse");
            assert!(
                err.to_string().contains(backend.name()),
                "error should name the backend: {err}"
            );
            assert!(
                err.to_string().contains("gaze config"),
                "error should say how to enable it: {err}"
            );
        }
    }

    #[test]
    fn each_backend_is_gated_independently() {
        assert!(ensure_backend_enabled(&config_with(true, false), Backend::Gnome).is_ok());
        assert!(ensure_backend_enabled(&config_with(true, false), Backend::KWallet).is_err());
        assert!(ensure_backend_enabled(&config_with(false, true), Backend::KWallet).is_ok());
        assert!(ensure_backend_enabled(&config_with(false, true), Backend::Gnome).is_err());
    }
}
