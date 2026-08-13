<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Console login (TTY)

Gaze can authenticate the `login` prompt on a virtual terminal, the one you get
on a server with no desktop or by switching to a free VT with Ctrl+Alt+F3.

Type your username, press Enter, and look at the camera. The password prompt is
printed by `pam_unix`, not by `login` itself, so when Gaze succeeds first that
prompt never appears and you go straight to a shell.

## Setup

`login` uses the shared authentication stack, so on most distros enabling Gaze
the usual way covers it:

::: code-group

```bash [Debian/Ubuntu]
sudo pam-auth-update --package
```

```bash [Fedora and compatible]
sudo authselect select gaze with-silent-lastlog --force
```

:::

Debian's `/etc/pam.d/login` includes `common-auth` and Fedora's includes
`system-auth`, so the profile you enable for `sudo` reaches the console too.

### Arch Linux

Arch needs a manual step. Gaze deliberately stays out of
`/etc/pam.d/system-auth` because `pambase` overwrites that file on upgrade (see
[PAM](/guide/pam#arch-linux-manjaro)), and `/etc/pam.d/login` reaches Gaze only
through that shared stack. Add Gaze to `/etc/pam.d/login` directly instead:

```bash
sudo awk '
    /^[[:space:]]*auth[[:space:]]/ && !done {
        print "auth        sufficient    pam_gaze.so"
        done = 1
    }
    { print }
' /etc/pam.d/login | sudo tee /tmp/pam-login-new && \
  sudo install -m 644 /tmp/pam-login-new /etc/pam.d/login
```

::: warning
Keep a root shell open while you test this. A mistake in `/etc/pam.d/login` can
lock you out of every virtual terminal.
:::

## Camera at the login prompt

No session exists before you log in, so there is no PipeWire to capture through
and no ACL granting your user the camera. Gaze notices that the seat has no
active session and reads its V4L2 device directly, as root.

Two consequences:

- Pinning `cameras.rgb` to a `pipewiresrc` pipeline will not work here. Use
  `usb:VVVV:PPPP` or leave it as `primary`. See
  [Select Camera Source](/guide/configuration#select-camera-source).
- On SELinux systems `login` runs confined and may not be able to open
  `/dev/video*` without an extra policy module.

Gaze only takes the seat device when logind reports that the seat has **no**
active session. If someone is already logged in on the seat, their session owns
the camera and Gaze will not reach past it, so a console login for a different
user falls back to a password.

## Keyring

There is no password to hand to `pam_gnome_keyring` or `pam_kwallet` when you
authenticate with your face, so a keyring unlocked at login will instead prompt
you later. On a headless or server console this usually does not matter; on a
machine where you start a desktop from the TTY, it does.

## Turning it off

On Debian and Ubuntu, drop Gaze from the shared stack and re-add it only where
you want it:

```bash
sudo pam-auth-update --disable gaze gaze-simultaneous
```

Then follow
[Selective setup](/guide/pam#selective-setup-password-at-gdm-face-authentication-for-sudo-and-polkit).
On Arch, remove the `pam_gaze.so` line you added to `/etc/pam.d/login`.
