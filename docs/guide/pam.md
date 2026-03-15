# PAM Setup

This page explains how Gaze hooks into PAM authentication.

If you used the one-line installer, most of this should already be in place. This page is mainly for users who installed manually or want to understand what was changed.

## Overview

Gaze ships PAM modules:

- `pam_gaze.so` (sequential)
- `pam_gaze_grosshack.so` (simultaneous mode)

It also installs `/etc/pam.d/gdm-face` for GNOME unlock/login integration.

In practice, the PAM changes most users care about are for:

- unlock
- `sudo`
- polkit prompts

This is usually not about shell logins.

Before changing PAM manually:

- Keep a root shell open while testing.
- Make a backup of any PAM file you edit.
- Do not remove password login until you have confirmed face auth works.

## Debian / Ubuntu

### What package install does

- Installs PAM module under system PAM library path
- Installs `pam-auth-update` profiles (`gaze`, `gaze-simultaneous`)
- Postinstall runs `pam-auth-update --package`

### How to enable or re-apply it

Run:

```bash
sudo pam-auth-update --package
```

You should see Gaze in the list of available authentication methods.

Choose:

- `Gaze Face Authentication (Sequential)` for standard behavior
- `Gaze Face Authentication (Simultaneous)` if you want face auth to run alongside the password prompt

### How to check that it worked

Run:

```bash
gaze auth --verbose
```

If face authentication starts and you see live matching output, the PAM pieces are installed and the daemon is reachable.

If login still does not use Gaze, re-run:

```bash
sudo pam-auth-update --package
```

## Fedora / RHEL

### What package install does

- Installs PAM module under `/usr/lib64/security`
- Installs authselect profile at `/usr/share/authselect/vendor/gaze`

### Enable the authselect profile

```bash
sudo authselect select vendor/gaze --force
```

If you want face auth to run at the same time as the password prompt:

```bash
sudo authselect select vendor/gaze with-face-simultaneous --force
```

### How to check that it worked

Run:

```bash
authselect current
gaze auth --verbose
```

What this means:

- `authselect current` should mention `vendor/gaze`
- `gaze auth --verbose` should open the camera and attempt a match

## Systems without `authselect` or `pam-auth-update`

Some distros do not have a helper tool that automatically wires Gaze into the shared PAM stack.

On those systems, you usually need to edit the common authentication stack that is used by unlock, `sudo`, and polkit.

### What you are trying to change

Find the shared PAM file that other services include for authentication.

Then add Gaze before the `pam_unix.so` line.

Typical pattern:

```text
auth    sufficient    pam_gaze.so
auth    sufficient    pam_unix.so try_first_pass nullok
```

If you want simultaneous behavior instead:

```text
auth    sufficient    pam_gaze_grosshack.so
auth    sufficient    pam_unix.so try_first_pass nullok
```

### Where this usually matters

You are normally aiming at the shared auth stack used by:

- screen unlock
- `sudo`
- polkit prompts

The exact file name varies by distro.

### Safer manual workflow

1. Identify the shared PAM auth file used by your distro.
2. Make a backup of it.
3. Add the Gaze line before `pam_unix.so`.
4. Keep an existing root shell open while testing.
5. Test with `gaze auth --verbose` and a real unlock or `sudo` prompt.

Example backup command:

```bash
sudo cp /etc/pam.d/system-auth /etc/pam.d/system-auth.bak
```

If your distro does not have `/etc/pam.d/system-auth`, use the equivalent shared auth file instead.

If the change breaks authentication, restore the backup immediately.

## Validate end-to-end auth

After PAM setup, run:

```bash
gaze add-face default
gaze auth --verbose
systemctl status gazed
```

What to expect:

- `gaze add-face default`: completes a capture session successfully
- `gaze auth --verbose`: shows scores and ideally authenticates you
- `systemctl status gazed`: service is active

## Safety notes

- Keep at least one fallback login method enabled (password, fingerprint, or console access).
- Avoid editing PAM files blindly without a tested recovery path.
