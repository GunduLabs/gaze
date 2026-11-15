<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Getting Started

Install Gaze, check your camera, enroll a face, then verify where face unlock is available.

## Before you begin

- A Linux desktop with a working webcam and `sudo` access
- AVX2 on x86_64 systems
- A usable TPM 2.0 for the default encrypted face storage
- Internet access for the first model download

If you have no TPM, the installer explains how to explicitly choose unencrypted storage. It never silently turns encryption off for a fresh installation.

## Install

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

Follow the installer's final status. If it asks for a reboot, reboot **before** continuing. GNOME needs a new session to load its extension; staged OSTree packages need a reboot. On Cinnamon, restart the desktop if requested. Otherwise you can continue immediately.

For manual packages, follow the [installation guide](/guide/installation).

## Set up your face

From your desktop session, as your normal user:

```bash
gaze setup
```

Setup checks camera frames, lets you choose another camera, enrolls a profile, tests recognition, and checks desktop integration. Recommended security settings are already selected; you do not need to tune thresholds first.

For a camera preview, open `gaze-gui`. Follow its setup checklist: choose a camera in **Settings → Everyday**, enroll a face, press **Test**, then **Check desktop integration**.

During enrollment, face the camera naturally for the reference capture. Make small movements in the requested directions, keeping your face centered and well lit.

## Try screen unlock

A successful recognition test confirms that Gaze can match your face. Desktop integration is checked separately. Resolve the errors and warnings from setup before testing your lock screen.

On KDE or hyprlock, set a 3–5 second start delay in Settings if locking immediately unlocks again while you are still in front of the camera. Login-screen and keyring unlock are separate, optional features; see your [desktop guide](/guide/pam).

## Change something later

```bash
gaze config                         # choose a settings section, review, and save
gaze config get security.level      # read one setting
gaze config set auth.start_delay_ms 3000
gaze test                           # concise recognition result
gaze test --verbose                 # detailed metrics
gaze doctor                         # actionable setup problems
gaze doctor --verbose               # all checks, including optional features
```

Changing between recognition models (for example Medium → High security) requires new captures for existing profiles. Use **Improve this face** or `gaze refine-face default`, then test again.

## If setup cannot start

Run `gaze doctor`. If the daemon cannot start, inspect `journalctl -u gazed -n 50`. A fresh installation without a usable TPM needs TPM enabled in firmware, or an explicit choice of plaintext storage in `/etc/gaze/config.toml` with `sudoedit`. Do not use `gaze config` to repair a daemon that is down. See [troubleshooting](/guide/troubleshooting) and [storage configuration](/guide/configuration#encrypt-face-templates-with-the-tpm).
