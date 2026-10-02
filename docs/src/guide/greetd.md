<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# greetd

greetd is a minimal login manager that hands the login screen to a separate
greeter such as tuigreet, ReGreet or noctalia-greeter. Gaze authenticates there
through the shared PAM stack, the same way it does on LightDM.

## Setup

greetd's PAM service includes the shared authentication stack, so enabling Gaze
the usual way is enough:

::: code-group

```bash [Debian/Ubuntu/Mint]
sudo pam-auth-update --package
```

```bash [Fedora and compatible]
sudo authselect select gaze with-silent-lastlog --force
```

```bash [openSUSE Tumbleweed]
sudo pam-config --add --gaze
sudo pam-config --update
```

:::

Some distributions also ship a `greetd-greeter` service. That stack runs the
greeter program itself, not your login, and Gaze never releases a keyring
credential to it.

Confirm with `gaze doctor`, then log out and authenticate with your face.

## GNOME Keyring unlock

A face login supplies no password, so `pam_gnome_keyring.so` has nothing to
unlock the keyring with and prompts once the desktop is up. Gaze can hand it a
TPM-protected copy of your password instead, as it does for
[GDM](/guide/gnome#optional-tpm-backed-keyring-unlock).

Turn on template encryption, liveness and GNOME Keyring unlock in
`gaze config`, then enroll the password:

```bash
sudo gaze config
sudo systemctl restart gazed
gaze keyring
```

Re-enroll after a TPM clear or a change to the account or keyring password.

### The PAM edit

`/etc/pam.d/greetd` already has a keyring line, but it does not take the token:

```pam
auth       optional    pam_gnome_keyring.so
```

Add `use_authtok` and leave the line where it is:

```pam
auth       optional    pam_gnome_keyring.so use_authtok
```

Keep the existing `session optional pam_gnome_keyring.so auto_start` line. Gaze
does not edit this file for you because it belongs to the distribution.

Unlike `gdm-face`, greetd uses one stack for both face and password logins, so
there is no `pam_deny` gate. The keyring line only has to come after
`system-auth`. A face match hands it the stored password, a typed password
hands it what you typed, and with neither it does nothing.

Do not copy the `pam_gaze.so` line from `gdm-face` or hyprlock into this file.
`[success=1 ...]` would skip the keyring line itself, and `sufficient` or
`[success=done ...]` end the auth section before it is reached.

`sudo gaze doctor` checks the order, not just the option.

### What this changes about your security

Everything in the
[GDM security notes](/guide/gnome#what-this-changes-about-your-security)
applies. In addition, greetd's stack is the full session stack rather than a
face-only one, so after a face match the password is visible to the modules
that follow the keyring line. On Fedora that is `postlogin` and, on a KDE
install, the `pam_kwallet5.so` and `pam_kwallet.so` lines.

### SELinux

On Fedora the greeter needs the `gaze-greeter-keyring` policy module to read the
stored record. `gaze keyring` loads it, and `sudo gaze doctor` reports it as
**Keyring SELinux policy**. Without it the login still succeeds and the keyring
prompts as before.

## Troubleshooting

`gkr-pam: no password is available for user` in `journalctl -b` means the
keyring line is missing `use_authtok`, sits before `system-auth`, or is skipped
by a `pam_gaze.so` line above it. `sudo gaze doctor` reports all three under
**Keyring**.
