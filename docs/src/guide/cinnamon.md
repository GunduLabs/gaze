<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Cinnamon Extension

Gaze lock screen and PolKit elevation confirmation for Linux Mint and Cinnamon desktops require the `gaze-cinnamon-extension`.

This Cinnamon Spices extension hooks into Cinnamon's internal PolKit authentication agent and screen shield dialogs.

> [!IMPORTANT]
> If you enable `require_confirmation_lock_screen = true` or `require_confirmation_elevation = true` in `/etc/gaze/config.toml`, the Gaze Cinnamon Extension **must** be enabled for face-authorization confirmation to function inside Cinnamon's graphical PolKit elevation prompts and on the lock screen.
> 
> **Why this is required:** Standard Cinnamon PolKit prompt windows do not natively allow confirming with an empty or blank password field. The Cinnamon Extension solves this by dynamically intercepting Gaze's confirmation signals, automatically hiding the password entry, displaying `"Face verified. Press Enter or click Authenticate to confirm."`, and focusing the native "Authenticate" button. On the lock screen, it provides a dedicated `"Confirm Face Unlock"` button and responds to Enter key presses without requiring password input.
> 
> If the extension is **inactive/disabled** while either toggle is set, Gaze's PAM modules will **safely bypass confirmation** (returning success instantly upon face match) to prevent empty input hangs and user lockouts.

## Installation and Enabling

To install the extension for your user account:

```bash
mkdir -p ~/.local/share/cinnamon/extensions/gaze@gundulabs.com
cp -r cinnamon-extension/* ~/.local/share/cinnamon/extensions/gaze@gundulabs.com/
```

Then enable the extension in Cinnamon:

1. Open **System Settings** → **Extensions**.
2. Locate **Gaze** in the list.
3. Click **+** (Add) to activate the extension.

Alternatively, restart Cinnamon by pressing `Alt + F2`, typing `r`, and pressing `Enter`.

## Open the extension preferences

Open **System Settings** → **Extensions**, find **Gaze**, and click the **Configure** (gear) button.

The configuration window provides:

| Setting | Contains |
|---|---|
| **Face authentication** | `Enable face authentication (lock screen)`, `Face retry mode`, `Maximum face tries`. Applies to this session's lock screen only. |

## Retry behavior

The extension decides how many times face authentication is retried within one authentication cycle. Both settings live under **Face authentication** in the extension settings:

| Setting | Key | Values | Default |
|---|---|---|---|
| Face retry mode | `face-retry-mode` | `disabled`, `fixed`, `infinite` | `fixed` |
| Maximum face tries | `max-face-tries` | 2 to 20 | 3 |

- `disabled`: one attempt. After it fails, face auth stops for that cycle and you finish with your password.
- `fixed`: retries until `max-face-tries` failures, then stops for that cycle.
- `infinite`: keeps retrying for as long as the prompt is open. The password entry stays usable throughout.

`max-face-tries` only applies in `fixed` mode, and the extension clamps it to a minimum of 2 even if a lower value is configured.

## PolKit Elevation Confirmation

When an administrative application (e.g. `gparted`, Software Sources, or Gaze configuration) requests elevated authentication:
1. Gaze senses your face in the background.
2. Upon a successful face match, the extension automatically hides the password input, updates the prompt label to `"Face verified. Press Enter or click Authenticate to confirm."`, and focuses the **Authenticate** button.
3. Press **Enter** or click **Authenticate** to confirm and proceed without typing your password.

## Lock Screen Authentication

On Cinnamon 6+ systems utilizing the internal screen shield, Gaze face authentication displays status updates (`Please look at the camera...`, `Hold still...`) directly on the lock screen. When confirmation is required:
1. A **Confirm Face Unlock** button appears with an active progress indicator.
2. Pressing **Enter** or clicking the button unlocks the session.
3. Pressing **Escape** cancels confirmation and returns to the standard password entry.
