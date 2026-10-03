// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use gaze_core::config::Config;
use gaze_security::keyring::{Backend, Zeroizing};

pub fn enroll(username: &str, config: &Config, backend: Backend) -> anyhow::Result<()> {
    anyhow::ensure!(
        match backend {
            Backend::Gnome => config.storage.unlock_gnome_keyring,
            Backend::KWallet => config.storage.unlock_kwallet,
        },
        "enable {} unlock with gaze config first",
        backend.name()
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
