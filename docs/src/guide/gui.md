<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# GUI Guide

::: tip On KDE Plasma
`gaze-kde` adds a **Face Unlock** entry to System Settings that opens this same app,
so you can reach it from where Plasma users expect to find it. See the
[KDE Plasma guide](/guide/kde#system-settings).
:::

`gaze-gui` is the easiest way to enroll faces and check auth health.

Launch it:

```bash
gaze-gui
```

- **Enroll a new face profile**: Initiates a guided camera capture. If both RGB and IR cameras are configured, it captures from both.
- **View enrolled profiles**: The main window lists enrolled faces with `RGB` and `IR` badges and the total template capture count. A badge is green when the profile has captures for that spectrum, amber when a camera is configured for it but the profile has none, and grey when no camera is configured for that spectrum at all. An RGB-only machine therefore shows a green `RGB` and a grey `IR`, not a failure.
- **Refine profiles**: Tap the edit/refine icon on a profile to capture additional samples or add a missing spectrum (e.g. adding IR captures to an existing RGB-only face profile after configuring an IR camera).
- **Test authentication**: Check Gaze's recognition with immediate pass/fail visual feedback.
- **Remove profiles**: Delete specific face profiles.
- **Configure daemon settings**: Change security levels, cameras, liveness settings, and hybrid policies.

## Configuration dialog

Open the config dialog from the header-bar settings button.

The dialog mirrors `/etc/gaze/config.toml`, grouped the same way. Saving writes
the file through the daemon, so it needs a polkit authorization.

**Security**

- Security level (`low`, `medium`, `high`, `maximum`, or `custom`)
- For `custom`: detector level, recognizer level, RGB and IR similarity thresholds

**Hardware**

- Inference execution provider, either ONNX Runtime directly or through OpenVINO
- OpenVINO inference device

Both offer only `cpu` on the released packages. The other values need a build
compiled with the `openvino-config` Cargo feature. See
[Configuration](/guide/configuration) for what those builds accept.

**Cameras**

- RGB camera source, IR camera source, and Force IR Emitter
- Parallel RGB + IR Capture, which decides whether hybrid verification reads both
  sensors at once. Some webcams cannot, so the default captures them one at a time
- Darkness cutoff, the dark-frame rejection threshold

**Enrollment**

- Max templates per face
- Minimum face size ratio, where lower values allow enrollment from farther away

**Liveness Anti-Spoofing**

- Enable liveness spoof prevention, liveness threshold, liveness max seconds

**Auth**

- Abort if SSH, abort if lid closed
- Require a suspend first, which refuses face auth until the machine has suspended
  and resumed once
- Require confirmation on lock screen, require confirmation for elevated auth
- Resume grace period and start delay, both in milliseconds
- Start delay applies to, either every face auth or screen lockers only
- Hybrid combining policy, used when both RGB and IR are enrolled

**Storage**

- Encrypt face templates, which seals enrolled templates with the TPM. See
  [How it works](/guide/how-it-works) for what that protects against.
- Unlock GNOME Keyring, which replays an enrolled password after a
  liveness-protected GDM face login. It needs template encryption and liveness
  on, and each user still has to run `gaze keyring`. Read
  [what it changes about your security](/guide/gnome#what-this-changes-about-your-security)
  first.

## Common tasks

1. Enroll a profile named `default`.
2. Run test authentication several times in normal room light.
3. Add another profile if your appearance varies often (for example, glasses).

## When to use GUI vs CLI

- Use GUI for enrollment and quick pass/fail checks.
- Use CLI (`gaze auth --verbose`) when you want detailed authentication metrics and diagnostics.

## If the GUI cannot authenticate

Check daemon status:

```bash
systemctl status gazed
```

If stopped:

```bash
sudo systemctl enable --now gazed
```

Then retry from GUI.
