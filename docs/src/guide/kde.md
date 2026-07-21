# KDE Plasma (lock screen)

On the KDE Plasma lock screen and SDDM greeter, Gaze's base install wires face
authentication into the interactive `kde` PAM stack, which only runs when you
submit the password field. That means face unlock does not start on its own;
you have to press Enter (even with an empty field) to trigger it.

The `gaze-kde` package fixes the flow. KScreenLocker also runs a separate
`kde-fingerprint` PAM service **noninteractively and up front**, in parallel
with the password field (the same slot the fingerprint reader uses).
`gaze-kde` installs `/etc/pam.d/kde-fingerprint` so that slot runs `pam_gaze`,
and face unlock begins the moment the lock screen appears, no key press needed.

## Install

::: code-group

```bash [Debian/Ubuntu]
sudo apt-get install gaze-kde
```

```bash [Fedora and compatible]
sudo dnf install gaze-kde
```

```bash [Arch]
yay -S gaze-kde-bin
```

:::

Lock your screen (or log out to the greeter) and look at the camera. Face
unlock runs while the password field waits; whichever succeeds first wins.

## How it works

KScreenLocker's greeter starts every PAM authenticator at once: the interactive
`kde` service for the password, plus the noninteractive `kde-fingerprint` and
`kde-smartcard` services. `gaze-kde` supplies a `kde-fingerprint` service that
runs `pam_gaze.so`:

1. The greeter calls PAM with service name `kde-fingerprint` as soon as it opens
2. `pam_gaze.so` claims the camera through the `gazed` DBus service and runs face verification
3. On match → `PAM_SUCCESS`, the screen unlocks
4. On no match → `PAM_AUTH_ERR`, the greeter keeps waiting on the password field
5. On no enrolled faces or an unavailable camera → `PAM_AUTHINFO_UNAVAIL`, the greeter drops the face option and shows the password field alone

The password path is never blocked; it runs in its own `kde` stack the whole
time.

## require_confirmation

The KDE lock screen has no channel to answer a confirmation prompt, so with
`require_confirmation = true` the standard module lets the face match unlock on
its own there. If you want the confirmation step enforced, KDE is not able to
present it on this path.

## Prerequisites

- `gazed` daemon running (`systemctl status gazed`)
- At least one enrolled face: `gaze add-face default`
- Working camera (test with `gaze auth`)

## Existing kde-fingerprint

If `/etc/pam.d/kde-fingerprint` already exists (for example from a fingerprint
reader setup), the package manager keeps your file and drops the packaged one
beside it as `.rpmnew` / `.pacnew` (or prompts, on Debian/Ubuntu). Add this line
to the top of the `auth` stack in your file to enable face unlock alongside the
reader:

```
auth        sufficient    pam_gaze.so
```

## Disable

::: code-group

```bash [Debian/Ubuntu]
sudo apt-get remove gaze-kde
```

```bash [Fedora and compatible]
sudo dnf remove gaze-kde
```

```bash [Arch]
yay -R gaze-kde-bin
```

:::

## Troubleshooting

- **Face unlock still waits for a password submit**: `kde-fingerprint` is not running `pam_gaze`. Run `gaze doctor` and check the "KDE lock screen" line, then reinstall `gaze-kde`.
- **Falls back to password every time**: daemon may not be running, or no faces enrolled for the current user. Check `systemctl status gazed` and `gaze list-faces`.
- **Camera busy**: another Gaze client (GUI, GNOME extension) may hold the camera. Close it and retry.
