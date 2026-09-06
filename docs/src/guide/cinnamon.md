<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Cinnamon Extension

Gaze lock screen and PolKit elevation integration are Cinnamon-specific and require the `gaze-cinnamon-extension` package. The one-line installer tries to enable the extension for the current Cinnamon user. Manual package installs only install the extension files. On openSUSE Tumbleweed, install the extension with `sudo zypper install gaze-cinnamon-extension` before enabling it.

This Cinnamon Spices extension hooks into Cinnamon's internal PolKit authentication agent and native unlock dialogs.

You do not need to enable this extension for the CLI, the GUI, or normal PAM prompts such as `sudo`. Leave it disabled on non-Cinnamon desktops.

> [!IMPORTANT]
> If you enable `require_confirmation_lock_screen = true` or `require_confirmation_elevation = true` in `/etc/gaze/config.toml`, this Cinnamon Extension **must** be enabled for face-authorization confirmation to function inside Cinnamon's graphical PolKit prompts and on the lock screen.
> 
> **Why this is required:** Standard Cinnamon PolKit prompt windows do not natively allow clicking confirmation buttons with an empty or blank password field. The Cinnamon Extension solves this by dynamically intercepting Gaze's confirmation signals, automatically hiding the password entry, and focusing the confirmation button (the native "Authenticate" button in PolKit and a dedicated "Confirm Face Unlock" button on the unified lock screen dialog).
> 
> If the extension is **inactive/disabled** under Cinnamon while either toggle is set, Gaze's PAM modules will **safely bypass confirmation** (returning success instantly upon face match) to prevent empty input hangs and user lockouts.

## Should I enable it?

Enable it if you use Cinnamon and want graphical PolKit elevation confirmation and face unlock from the lock screen.

Do not enable it if you only want CLI/GUI enrollment, normal PAM authentication, or you are not using Cinnamon.

## Enable the extension

If the package is installed but the extension is not enabled yet, from your Cinnamon session:

```bash
gsettings set org.cinnamon enabled-extensions \
  "$(gsettings get org.cinnamon enabled-extensions | sed "s/]\$/, 'gaze@gundulabs.com']/; s/^@as \[\]\$/['gaze@gundulabs.com']/")"
```

Then reload Cinnamon by pressing `Alt + F2`, typing `r`, and pressing `Enter` (or run `cinnamon --replace &` from a terminal).

You can also enable it graphically:
1. Open **System Settings** → **Extensions**.
2. Locate **Gaze** in the list.
3. Click **+** (Add) to activate the extension.

## Open the extension preferences

```bash
cinnamon-settings extensions
```

Or open **System Settings** → **Extensions**, find **Gaze**, and click the **Configure** (gear) button from the row.

The configuration window provides:

| Group | Contains |
|---|---|
| **Face authentication** | `Enable face authentication (lock screen)`, `Face retry mode`, `Maximum face tries`. Applies to the Cinnamon session. |

## Retry behavior

The extension decides how many times face authentication is retried within one
authentication cycle. All settings live under **Face authentication** in the extension preferences:

| Setting | Key | Values | Default |
|---|---|---|---|
| Face retry mode | `face-retry-mode` | `disabled`, `fixed`, `infinite` | `fixed` |
| Maximum face tries | `max-face-tries` | 2 to 20 | 3 |

- `disabled`: one attempt. After it fails, face auth stops for that cycle and you
  finish with your password.
- `fixed`: retries until `max-face-tries` failures, then stops for that cycle.
- `infinite`: keeps retrying for as long as the prompt is open. The password entry
  stays usable throughout.

`max-face-tries` only applies in `fixed` mode, and the extension clamps it to a
minimum of 2 even if a lower value is configured.

## Create a face profile

Enrollment does not live in the extension preferences. Use the Gaze settings app or the CLI:

```bash
gaze-gui             # Faces list, press + to enroll
gaze add-face default
```

The profile name defaults to `default`, matching the CLI quick-start flow. Follow the camera prompts until the profile is saved.

## PolKit elevation confirmation

When an administrative application (e.g. `gparted`, Software Sources, or Gaze configuration) requests elevated authentication:
1. Gaze senses your face in the background.
2. Upon a successful face match, the extension automatically hides the password input, updates the prompt label to `"Face verified. Press Enter or click Authenticate to confirm."`, and focuses the **Authenticate** button.
3. Press **Enter** or click **Authenticate** to confirm and proceed without typing your password.

## Lock screen behavior

How Gaze behaves on the lock screen depends on your Cinnamon desktop version and session type:

### Unified Native Screensaver (Wayland / Cinnamon 6.8+)

On Cinnamon sessions using the integrated native screen shield (introduced in Cinnamon 6.8+ and Wayland sessions), the lock screen runs natively within Cinnamon's window manager process (`imports.ui.screensaver.unlockDialog`). The Gaze Cinnamon Extension hooks directly into this dialog:

1. **Confirmation Button**: When `require_confirmation_lock_screen = true`, a **Confirm Face Unlock** button appears with an active progress spinner.
2. **Key Navigation**: Pressing **Enter** or clicking the button confirms and unlocks the session. Pressing **Escape** cancels confirmation and returns to the password field.
3. **Status Feedback**: Interactive camera prompts (`Please look at the camera...`, `Hold still...`, `Need more light...`) display directly on the lock screen label.
4. **Configurable Retries**: Follows the configured `face-retry-mode` and `max-face-tries` settings.

### Standalone Screensaver (`cinnamon-screensaver` / Linux Mint 22.x on X11)

In Linux Mint 22.x (and standard X11 sessions), screen locking is handled by the standalone `/usr/bin/cinnamon-screensaver` process (written in Python/GTK3), which runs outside Cinnamon's CJS environment.

- **Hands-Free Face Unlock**: When `require_confirmation_lock_screen = false`, Gaze verifies your face via PAM (`/etc/pam.d/cinnamon-screensaver`) and unlocks the screen immediately on match.
- **Confirmation UI**: Because `/usr/bin/cinnamon-screensaver` is a separate GTK application and does not support CJS Spices extensions, custom button widgets cannot be rendered onto its unlock window. Elevation prompts (PolKit) remain fully supported with the confirmation UI across all Cinnamon versions.

## Verify Cinnamon flow

- **Test PolKit elevation**: Open a root-authorized tool (or run `pkexec id` in a graphical terminal). Look at the camera; once your face is verified, confirm with Enter or click Authenticate without typing a password.
- **Test Lock Screen**: Lock your screen (`Super + L` or `Ctrl + Alt + L`), wake the display, and look at the camera to unlock.
