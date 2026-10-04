<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Getting Started

Get Gaze up and running in under 10 minutes. Install it, enroll your face, and test authentication.

## Before you begin

- Linux desktop with a working PipeWire/GStreamer webcam
- `sudo` access
- Internet connection for first-time model download

## Step 1: Install Gaze

The recommended option is the one-line installer:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

If you prefer manual package setup, use the [installation guide](/guide/installation).

## Step 2: Check daemon status

```bash
systemctl status gazed
```

If the service is not running, enable and start it:

```bash
sudo systemctl enable --now gazed
```

## Step 3: Enroll your first face

```bash
gaze add-face default
```

Tips while enrolling:

- Keep your face centered and well lit.
- Face the camera naturally for the first capture; the remaining direction
  prompts are measured relative to that pose.
- Use small, deliberate movements and hold each prompted angle still.
- Remove strong backlight if possible.

## Step 4: Test authentication

```bash
gaze auth
```

To see more detail about the authentication attempt, add `--verbose`:

```bash
gaze auth --verbose
```

## Step 5: Open the GUI (optional)

```bash
gaze-gui
```

Use the GUI to enroll additional face profiles (for example, with glasses and without glasses).

## Step 6: Verify GNOME lock screen auth (optional)

Follow these steps only if you use GNOME and want face unlock on the lock screen. The one-line installer enables the extension for your current GNOME user when possible. If you installed the packages manually, or automatic enablement failed, run:

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

Run these commands from a GNOME session that started **after** the package was installed. GNOME Shell scans extension directories when a session starts and drops extension IDs it has not seen. If you enable the extension in the session where you installed it, the setting may appear to work but disappear after you log out. Reboot, then run the commands again. `gaze doctor` reports this as `GNOME extension: installed, but not enabled for the current user` and prints the same steps. See [The extension disappears again after a logout](/guide/gnome#the-extension-disappears-again-after-a-logout).

Hands-free lock screen and GDM login authentication through this extension are specific to GNOME. Cinnamon has a separate extension for its lock screen and PolKit prompts; see the [Cinnamon guide](/guide/cinnamon). KDE Plasma uses the biometric PAM slot that KScreenLocker starts in advance; see the [KDE Plasma guide](/guide/kde). Other login surfaces use PAM integrations. See the guides for [Hyprland](/guide/hyprland), [LightDM](/guide/lightdm), [console login (TTY)](/guide/console), and [PAM](/guide/pam).
GDM login face auth is separate and disabled by default due to GNOME keyring behavior.
See [GNOME Extension](/guide/gnome) for details and optional login enablement, including the optional TPM-backed keyring unlock that removes that caveat.

## If something fails

Go to the [troubleshooting guide](/guide/troubleshooting) for camera, daemon, PAM, and low-match issues.

## Next

- Tune behavior in the [configuration guide](/guide/configuration)
- Learn commands in the [CLI guide](/guide/cli)
- Use the desktop app via the [GUI guide](/guide/gui)
- Review PAM setup in [PAM](/guide/pam)
- Review lock/login behavior in [GNOME Extension](/guide/gnome)
- Set up the [Cinnamon Extension](/guide/cinnamon) for the Cinnamon lock screen and PolKit prompts
- Set up the [KDE Plasma](/guide/kde) lock screen and System Settings page
- Enable face unlock for [Hyprland (hyprlock)](/guide/hyprland)
- Add face auth to [LightDM](/guide/lightdm) or a [console login (TTY)](/guide/console)
