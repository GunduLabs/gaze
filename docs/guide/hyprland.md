# Hyprland (hyprlock)

Gaze integrates with [hyprlock](https://github.com/hyprwm/hyprlock), the Hyprland screen locker, via a dedicated PAM service. The `gaze-hyprlock` package installs `/etc/pam.d/hyprlock-gaze`, leaving the distro's own `/etc/pam.d/hyprlock` untouched.

You opt in by pointing `hyprlock.conf` at the Gaze PAM service.

## Install

The one-line installer auto-installs `gaze-hyprlock` and edits `~/.config/hypr/hyprlock.conf` when it detects a Hyprland session or the `hyprlock` binary.

Manual install:

::: code-group

```bash [Debian/Ubuntu]
sudo apt-get install gaze-hyprlock
```

```bash [Fedora]
sudo dnf install gaze-hyprlock
```

```bash [Arch]
yay -S gaze-hyprlock-bin
```

:::

## Enable

Add to `~/.config/hypr/hyprlock.conf`:

```ini
general {
    pam_module = hyprlock-gaze
}
```

Restart hyprlock or lock the session again. Face unlock runs first; if it fails or times out, hyprlock falls back to the password prompt.

## Simultaneous mode

For password-and-face authentication in parallel (type your password while the camera matches your face — whichever succeeds first unlocks), use:

```ini
general {
    pam_module = hyprlock-gaze-simultaneous
}
```

This uses `pam_gaze_grosshack.so`, a PAM shim that lets Gaze run alongside the password prompt instead of blocking it.

## How it works

`hyprlock-gaze` is a PAM service that stacks `pam_gaze.so` on top of your system password stack (`system-auth` on RPM/Arch, `common-auth` on Debian/Ubuntu). The auth flow:

1. hyprlock calls PAM with service name `hyprlock-gaze`
2. `pam_gaze.so` runs as the logged-in user, claims the camera via the `gazed` DBus service, and runs face verification
3. On match → `PAM_SUCCESS`, hyprlock unlocks
4. On no enrolled faces or daemon unavailable → `PAM_IGNORE`, hyprlock proceeds to password
5. On no match → `PAM_AUTH_ERR`, hyprlock proceeds to password

No `gazed` configuration changes are required. The DBus policy already permits unprivileged users to claim the camera and run verification against their own enrolled templates.

## Prerequisites

- `gazed` daemon running (`systemctl status gazed`)
- At least one enrolled face: `gaze add-face default`
- Working camera (test with `gaze auth`)

## Disable

Remove the `pam_module` line from `hyprlock.conf` (or set it to hyprlock's default, `hyprlock`). Uninstall the package if you do not plan to use it again:

::: code-group

```bash [Debian/Ubuntu]
sudo apt-get remove gaze-hyprlock
```

```bash [Fedora]
sudo dnf remove gaze-hyprlock
```

```bash [Arch]
yay -R gaze-hyprlock-bin
```

:::

## Troubleshooting

- **Falls back to password every time** — daemon may not be running, or no faces enrolled for the current user. Check `systemctl status gazed` and `gaze list-faces`.
- **Camera busy** — another Gaze client (GUI, GNOME extension) may hold the camera. Close it and retry.
- **PAM error in logs** — check `journalctl -u gazed` and `journalctl --user -t hyprlock`.
