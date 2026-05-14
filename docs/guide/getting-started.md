# Getting Started

Get Gaze running in under 10 minutes: install, enroll your face, and verify authentication.

## Before you begin

- Linux desktop with a working PipeWire/GStreamer webcam
- `sudo` access
- Internet connection for first-time model download

## Step 1: Install Gaze

Recommended one-line installer:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

If you prefer manual package setup, use the [installation guide](/guide/installation).

## Step 2: Check daemon status

```bash
systemctl status gazed
```

If it is not running:

```bash
sudo systemctl enable --now gazed
```

## Step 3: Enroll your first face

```bash
gaze add-face default
```

Tips while enrolling:

- Keep your face centered and well lit.
- Let it capture multiple angles.
- Remove strong backlight if possible.

## Step 4: Test authentication

```bash
gaze auth
```

For extra details:

```bash
gaze auth --verbose
```

## Step 5: Open the GUI (optional)

```bash
gaze-gui
```

Use the GUI to enroll additional face profiles (for example, with glasses and without glasses).

## Step 6: Verify GNOME lock screen auth (optional)

Only do this on GNOME if you want face unlock from the lock screen. The one-line installer enables the extension for the current GNOME user when possible. If you installed packages manually or the installer could not enable it automatically, run:

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

On Wayland, log out and back in after installing extension updates if the lock screen does not pick it up immediately.

Lock screen and GDM login integration are GNOME-only and require this extension.
GDM login face auth is separate and disabled by default due to GNOME keyring behavior.
See [GNOME Extension](/guide/gnome) for details and optional login enablement.

## If something fails

Go to the [troubleshooting guide](/guide/troubleshooting) for camera, daemon, PAM, and low-match issues.

## Next

- Tune behavior in the [configuration guide](/guide/configuration)
- Learn commands in the [CLI guide](/guide/cli)
- Use the desktop app via the [GUI guide](/guide/gui)
- Review PAM setup in [PAM](/guide/pam)
- Review lock/login behavior in [GNOME Extension](/guide/gnome)
