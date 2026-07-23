# KDE Plasma (lock screen)

On the KDE Plasma **lock screen** (KScreenLocker), the `gaze-kde` package makes
face unlock start on its own, no key press needed. KScreenLocker runs a separate
`kde-fingerprint` PAM service **noninteractively and up front**, in parallel with
the password field (the same slot the fingerprint reader uses). `gaze-kde` adds
`pam_gaze` to `/etc/pam.d/kde-fingerprint` so that slot runs face auth, and face
unlock begins the moment the lock screen appears.

> **Login greeter (SDDM) is separate.** This up-front trick is specific to
> KScreenLocker. SDDM does not start noninteractive PAM services on its own, so
> at the SDDM login greeter face auth behaves like the fingerprint reader on your
> SDDM build; typically it starts when you press Enter on an empty password
> field. See [SDDM login](#sddm-login) below.

The one-line installer auto-installs `gaze-kde` when it detects a KDE Plasma
session, so these steps are only needed for a manual install.

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
`kde-smartcard` services. `gaze-kde` inserts `pam_gaze.so` as the first `auth`
line of the `kde-fingerprint` service:

1. The greeter calls PAM with service name `kde-fingerprint` as soon as it opens
2. `pam_gaze.so` claims the camera through the `gazed` DBus service and runs face verification, streaming live hints ("Need more light…", "Hold still…") as PAM info messages the greeter displays
3. On match → `PAM_SUCCESS`, the screen unlocks
4. On no match (a face was seen but did not match) → the rest of the `kde-fingerprint` stack runs (e.g. a fingerprint reader), the greeter keeps waiting on the password field, and KScreenLocker may re-arm face so it keeps trying
5. On give-up (camera covered / too dark / no face within the daemon's short timeouts), an unavailable camera, or no enrolled faces → `PAM_AUTHINFO_UNAVAIL`, and KScreenLocker marks the face slot unavailable and **stops the camera** instead of re-triggering it

The password path is never blocked; it runs in its own `kde` stack the whole
time.

### Why the control string matters

KScreenLocker only re-arms a noninteractive authenticator that fails with an
ordinary error; one that reports `PAM_AUTHINFO_UNAVAIL` is dropped for the rest
of that lock. So the inserted line uses `authinfo_unavail=die` to make gaze's
give-up (too dark / no face) reach KScreenLocker as *unavailable*, so the camera
stops cleanly rather than looping. Everything else (`ignore`/`default=ignore`)
falls through to the fingerprint reader and password. The trade-off: a give-up
also releases the shared fingerprint slot until the next lock.

## require_confirmation

The KDE lock screen has no channel to answer a confirmation prompt, so with
`require_confirmation = true` the standard module lets the face match unlock on
its own there. If you want the confirmation step enforced, KDE is not able to
present it on this path.

## SDDM login

The SDDM login greeter is a different program from the lock screen and does not
have KScreenLocker's up-front `kde-fingerprint` slot. SDDM runs its PAM stack
when you submit the login form, so, exactly like a fingerprint reader on SDDM,
face auth starts when you **press Enter on an empty password field**. Whether it
can also start with no key press depends on your SDDM build's background
biometric support.

To enable it, add `pam_gaze` to the top of the SDDM login stack the same way you
would `pam_fprintd`:

```
# /etc/pam.d/sddm  (first auth line)
auth        [success=done authinfo_unavail=die ignore=ignore default=ignore]    pam_gaze.so
```

Then look at the camera and press Enter on the empty field to log in. This path
does not present the confirmation prompt (see below).

## Prerequisites

- `gazed` daemon running (`systemctl status gazed`)
- At least one enrolled face: `gaze add-face default`
- Working camera (test with `gaze auth`)

## Existing kde-fingerprint

On KDE, `/etc/pam.d/kde-fingerprint` is provided by `plasma-workspace`, so
`gaze-kde` does not own that file. Its postinstall script edits the file in
place, prepending

```
auth        [success=done authinfo_unavail=die ignore=ignore default=ignore]    pam_gaze.so
```

as the first `auth` line and leaving the rest of the stack (a fingerprint
reader, `pam_deny`, includes) intact as fallback. It is idempotent: a second
install does not add the line twice, an older gaze line (e.g. a previous
`default=ignore`) is migrated to the current control string in place, and
removing `gaze-kde` strips the line back out. If you prefer to wire it up by
hand, add that line yourself.

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
- **Camera keeps re-triggering / never stops when covered or in the dark**: you have an older `kde-fingerprint` line with `default=ignore`, which swallowed the give-up. Reinstall `gaze-kde` (or re-run `just dev-link-system`) to migrate the line to the current `authinfo_unavail=die` control string.
