<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Omarchy

The `gaze-omarchy` package integrates Gaze with Omarchy's Quickshell lock screen.
It provides separate face, password and fingerprint PAM conversations. A face
failure leaves the screen locked and password input available throughout the scan.

Gaze supports Omarchy **4.0.2 and 4.0.3**. Enablement checks the shell files
for compatibility. Earlier Omarchy releases using hyprlock should follow the
[Hyprland guide](/guide/hyprland).

## Install and enable

The [installer](/guide/installation) detects Omarchy running under Hyprland,
selects the Omarchy integration package and invokes `gaze-omarchy enable` as the
desktop user. It does not edit `hyprlock.conf` on Quickshell Omarchy.

Install manually on Arch with:

```bash
yay -S --needed gaze-bin gaze-gui-bin gaze-omarchy-bin
gaze config
gaze add-face default
gaze auth
gaze-omarchy enable
gaze-omarchy doctor
```

These commands are run from your unlocked desktop, without sudo. Enroll at your
normal working distance and follow Gaze's guided head movements. Existing Gaze
enrollments and camera/security configuration are reused.

The package installs its plugin under `/usr/share/gaze/omarchy`. Enablement links
it into the current user's Omarchy plugin directory, enables
`com.gundulabs.gaze.lock`, and restarts the shell only after checking that the
desktop is unlocked and the password PAM service is configured. An existing
custom lock plugin must be disabled first. Enablement preserves an unrelated
directory at the Gaze plugin path.

## Lock behavior

- One automatic scan is queued after the session lock becomes secure, with a
  three-second delay to give you time to step away.
- Waking a display blanked by the locker queues one fresh attempt. Ordinary
  mouse movement while the display is awake does not continuously restart scans.
- Enter on an empty password field retries. Escape cancels a queued or active
  scan. Typing stops the queued automatic attempt.
- Password input remains available during a face scan. Each attempt has a
  twelve-second outer timeout. Gaze's own scan and lockout limits still apply.
- The screen does not blank during a face attempt. Face failure text appears
  separately from password errors.
- If `auth.require_confirmation_lock_screen` is enabled, a recognized face
  prompts for Enter. The plugin responds to Gaze's specific confirmation prompt;
  it never forwards a password to the face conversation. PAM must still return
  success before the screen unlocks.

A visible enrolled face can unlock a newly locked session after the delay. For
explicit confirmation, enable `auth.require_confirmation_lock_screen` in
`gaze config`. The UI delay is additional to any configured daemon start delay;
longer daemon delays may exceed the plugin's timeout and require password unlock.

## Authentication boundary

The plugin authenticates against `/etc/pam.d/gaze-omarchy-face`, which is owned by
the Gaze Omarchy package. Only a successful `pam_gaze.so` result can authenticate
this service; missing enrollment, daemon errors, and `PAM_IGNORE` fall through
to `pam_deny.so`. A `pam_faillock` lockout blocks the face lane too. The service
does not include a password stack. Readiness uses
`HasEnrolledFaces` over the system bus and does not open the camera or verify a face.

Gaze's system daemon retains ownership of recognition, liveness, template
storage, remote-session checks and lid policy. Sudo and polkit use
[Gaze's PAM integration](/guide/pam).

RGB and IR camera support follows your Gaze configuration. IR alone is not a
depth sensor, and neither this plugin nor webcam liveness establishes Windows
Hello/Face ID equivalence. Actual recognition and presentation-attack resistance
depend on hardware, enrollment and policy.

## Updates and diagnosis

```bash
gaze doctor
gaze-omarchy doctor
```

These checks are read-only. The Omarchy check verifies host fingerprints,
dedicated PAM rules and ownership, enrollment, plugin enablement and the running
lock plugin identity/version. It does not use the public service map's `active`
flag, since Omarchy hides authentication services there.

After updating Gaze, reload from your unlocked desktop with:

```bash
gaze-omarchy enable
```

If an Omarchy update changes the shell files, the compatibility check reports
the difference and face scans stop. Unlock with your password and update
`gaze-omarchy`. Run `gaze-omarchy enable` from your unlocked desktop to reload it.

## Disable and uninstall

From your unlocked desktop:

```bash
gaze-omarchy disable
```

This enables the stock `omarchy.lock`, restarts the shell, checks that password
PAM is ready and Gaze is no longer the active lock plugin, and removes only the
package-owned symlink. Enrollments remain available to other Gaze clients.

Then remove `gaze-omarchy-bin` (or the source-built `gaze-omarchy` package). Repeat
disablement for each desktop user before removing a shared system package. The
package removal guard checks default `/home/*/.config` and `/root/.config`
locations; users with custom `XDG_CONFIG_HOME` locations must disable explicitly.
`gaze uninstall` restores the current user's Omarchy locker before removing Gaze
and stops if restoration fails.

