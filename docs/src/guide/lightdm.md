<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# LightDM

LightDM is the login screen on Linux Mint, Xubuntu, Ubuntu MATE, Ubuntu
Cinnamon, Linux Lite and Manjaro Xfce. Gaze can authenticate there before you
type anything, because LightDM runs the PAM stack as soon as a user is selected
rather than waiting for a password.

Whether that turns into a hands-free login depends on which greeter your distro
ships. See [Greeter support](#greeter-support).

## Setup

There is no `gaze-lightdm` package. LightDM's PAM service already includes the
shared authentication stack, so enabling Gaze the usual way is enough:

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

```bash [Arch]
# /etc/pam.d/lightdm includes system-login, which includes system-auth
```

:::

Debian's `/etc/pam.d/lightdm` is patched to `@include common-auth`, Fedora's is
`auth substack system-auth`, openSUSE's is `auth include common-auth`, and
Arch's is `auth include system-login`. In every case the profile or module you
enable for `sudo` reaches the login screen too.

Confirm with `gaze doctor`, then log out and select your user.

## Greeter support

LightDM itself authenticates you before you type. The greeters then decide
whether to start the session automatically or wait for a click, and they differ:

| Greeter | Distros | Hands-free |
|---|---|---|
| slick-greeter 2.2.7+ | Linux Mint, LMDE | Yes |
| slick-greeter older than 2.2.7 | Linux Mint 22 and earlier | No, press **Log In** |
| lightdm-gtk-greeter | Xubuntu, Manjaro Xfce | No, press **Log In** |

Mint relaxed the check in slick-greeter 2.2.7 so that a successful
authentication which produced a message, rather than a password prompt, logs you
straight in. Gaze emits such a message while it looks for your face, so on Mint
the session starts on its own.

`lightdm-gtk-greeter` still requires the user to have been prompted before it
will start a session, so face authentication succeeds but the greeter waits on
the **Log In** button. That is an upstream limitation, tracked in
[lightdm-gtk-greeter#140](https://github.com/Xubuntu/lightdm-gtk-greeter/issues/140);
it is not something Gaze can work around from a PAM module.

::: warning The password field waits for the camera
LightDM runs one authentication at a time, so while Gaze is looking for your
face the greeter will not accept a typed password. It is accepted as soon as the
face attempt finishes. This affects every biometric module on LightDM, not just
Gaze ([LP#1310104](https://bugs.launchpad.net/bugs/1310104)), and it is why
`auth.start_delay_ms` should stay low on this surface.

The greeter also does not cancel an authentication when the screen blanks
([lightdm-gtk-greeter#58](https://github.com/Xubuntu/lightdm-gtk-greeter/issues/58)),
so a scan started before the screen turned off runs to completion.
:::

## Camera at the login screen

Whether the greeter has a camera session depends on your systemd version. From
systemd 256 a greeter session gets its own user manager, so it has a PipeWire
socket like any other session and Gaze captures through it normally. On older
systemd, or where that manager does not start, there is no socket to bind to and
Gaze captures the seat's V4L2 device instead.

Both paths work without configuration. `gazed` opens the camera rather than the
greeter, so the greeter's own confinement is not in the way.

Pinning `cameras.rgb` to a `pipewiresrc` pipeline will not work on the V4L2 path.
Leave it as `primary`, which falls back to a V4L2 node when PipeWire cannot be
reached, or pin it to `usb:VVVV:PPPP` to skip the failed attempt. See
[Select Camera Source](/guide/configuration#select-camera-source).

Gaze uses the greeter's camera only while the greeter is the active session on
seat0, which is also the condition under which the greeter holds the camera's
access control entry. Two situations therefore have no greeter to authenticate
for:

- **Autologin.** With autologin configured, LightDM starts your session directly
  and never creates a greeter, so there is no login screen to authenticate at.
- **Seats other than seat0.** Gaze only looks at seat0, so a greeter on a second
  seat is not covered.

An already-open camera is not closed when a session stops being active, so a
program left running in a previous session can keep the device and capture may
fail as busy.

## Keyring and encrypted home

Logging in with your face means no password is available to unlock the GNOME
keyring or KWallet, so you will be asked for it after the session starts. This
is the same trade-off described in the
[GNOME login warning](/guide/gnome#login-warning-gnome-keyring). If you would
rather keep the keyring unlocking automatically, authenticate at the login
screen with your password and use Gaze for `sudo`, polkit and the lock screen.

::: danger Do not enable this with an encrypted home directory
If your home directory is unlocked by your login password, through `pam_ecryptfs`
or a similar module, face authentication at the login screen will fail to unlock
it. The session then opens and immediately closes, and LightDM returns to the
greeter and tries again, which looks like a login loop that face authentication
keeps restarting. Getting out of it means covering the camera until you are given
a password prompt.

Check for `pam_ecryptfs` in `/etc/pam.d/common-auth` before enabling Gaze at the
login screen. Encrypted home directories and face login at the greeter are not
compatible; use Gaze for `sudo`, polkit and the lock screen instead.
:::

## Turning it off

Remove Gaze from the shared stack, or keep it for elevation only:

```bash
sudo pam-auth-update --disable gaze gaze-simultaneous
```

On openSUSE Tumbleweed, use `pam-config` instead:

```bash
sudo pam-config --delete --gaze --gaze_grosshack
sudo pam-config --update
```

Then follow
[Selective setup](/guide/pam#selective-setup-password-at-gdm-face-authentication-for-sudo-and-polkit)
to re-add Gaze to `sudo` and polkit alone.
