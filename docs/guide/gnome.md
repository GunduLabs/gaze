# GNOME Extension

Gaze lock screen and GDM integration are GNOME-specific and require the `gaze-gnome-extension` package. The default installer installs this package, but does not enable the extension.

This extension starts the `gdm-face` PAM service inside GNOME Shell authentication flows.

You do not need to enable this extension for the CLI, the GUI, or normal PAM prompts such as `sudo`. Leave it disabled on non-GNOME desktops.

## Should I enable it?

Enable it if you use GNOME and want face unlock from the lock screen.

Do not enable it if you only want CLI/GUI enrollment, normal PAM authentication, or you are not using GNOME.

## Enable the extension

If the package is installed but the extension is not enabled yet:

```bash
gnome-extensions enable gaze@gundulabs.com
```

Then log out and back in once after install or update.

## Login warning (GNOME keyring)

Face authentication for the GDM login screen is disabled by default.

This is mostly about GNOME keyring behavior. GNOME keyring is normally unlocked by your login password. If you log in with face only, that password is never entered, so the keyring may stay locked.

When that happens, apps that read saved secrets (browser credentials, git credentials, Wi-Fi secrets, chat clients, etc.) can keep prompting for a keyring password until you unlock it manually.

## Optional: enable face at GDM login

If you still want this, enable it with:

```bash
sudo -u gdm dbus-run-session gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

Then restart GDM (or reboot):

```bash
sudo systemctl restart gdm
```

## Disable face at GDM login

```bash
sudo -u gdm dbus-run-session gsettings set org.gnome.shell.extensions.gaze enable-face-authentication false
```

## Verify GNOME flow

- Lock screen, then try unlock with face.
- If login face auth is enabled, test a full logout/login cycle.
