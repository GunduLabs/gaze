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

## Fedora / RHEL

Packages install an authselect profile at:

`/usr/share/authselect/vendor/gaze`

Enable it:

```bash
sudo authselect select vendor/gaze --force
```

Or simultaneous mode:

```bash
sudo authselect select vendor/gaze with-face-simultaneous --force
```

Verify profile + PAM behavior:

```bash
sudo authselect current
sudo -v
```

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
