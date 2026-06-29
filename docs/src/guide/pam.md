# PAM

This page is about normal PAM integration (`sudo`, polkit, shared auth stacks).

`gaze auth` is useful, but it is only a daemon/camera test. It does not run through PAM.

If you specifically want GNOME lock screen or GDM login behavior, use the [GNOME Extension guide](/guide/gnome).

## What Gaze installs

- `pam_gaze.so` (sequential mode, recommended)
- `pam_gaze_grosshack.so` (simultaneous mode)

Sequential means face auth runs first, then password fallback.
Simultaneous means face auth and password prompt run in parallel.

## Debian / Ubuntu

Packages install PAM profiles for `pam-auth-update`.

Apply or re-apply them:

```bash
sudo pam-auth-update --package
```

Pick one of the Gaze entries, then test with a real PAM prompt:

```bash
sudo -v
```

If camera opens and face auth runs, PAM wiring is active.

## Fedora / RPM systems

RPM packages install an authselect profile at:

`/usr/share/authselect/vendor/gaze`

Enable it:

```bash
sudo authselect select gaze with-silent-lastlog --force
```

Or simultaneous mode:

```bash
sudo authselect select gaze with-face-simultaneous with-silent-lastlog --force
```

Verify profile + PAM behavior:

```bash
sudo authselect current
sudo -v
```

## Arch Linux / Manjaro

The one-liner installer and the AUR package post-install script both configure `/etc/pam.d/sudo` automatically, inserting `pam_gaze.so` before the existing `auth include system-auth` line.

If you need to apply or re-apply it manually:

```bash
sudo awk '
    /^[[:space:]]*auth[[:space:]]/ && !done {
        print "auth        sufficient    pam_gaze.so"
        done = 1
    }
    { print }
' /etc/pam.d/sudo | sudo tee /tmp/pam-sudo-new && sudo install -m 644 /tmp/pam-sudo-new /etc/pam.d/sudo
```

Then test:

```bash
sudo -v
```

::: warning pambase updates
`/etc/pam.d/system-auth` is owned by the `pambase` package and gets overwritten on system upgrades. Gaze is added to `/etc/pam.d/sudo` directly to avoid this, but if you manually added `pam_gaze.so` to `system-auth` it will be lost on `pambase` updates.
:::

## Other distros (manual)

Edit your shared auth stack (for example `/etc/pam.d/system-auth`) and place Gaze before `pam_unix.so`.

Sequential:

```text
auth    sufficient    pam_gaze.so
auth    sufficient    pam_unix.so try_first_pass nullok
```

Simultaneous:

```text
auth    sufficient    pam_gaze_grosshack.so
auth    sufficient    pam_unix.so try_first_pass nullok
```

Then test with `sudo -v`.

## Safety notes

- Keep password auth enabled while testing.
- Keep a root shell open before changing PAM.
- Back up PAM files first so you can restore quickly.
