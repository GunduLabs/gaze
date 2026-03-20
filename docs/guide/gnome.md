# GNOME Extension

Gaze lock screen and GDM integration are GNOME-specific and require the `gaze-gnome-extension` package.

This extension starts the `gdm-face` PAM service inside GNOME Shell authentication flows.

## Enable the extension

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
sudo -u gdm dbus-run-session gsettings set org.gnome.login-screen.gaze enable-face-authentication true
```

Then restart GDM (or reboot):

```bash
sudo systemctl restart gdm
```

## Disable face at GDM login

```bash
sudo -u gdm dbus-run-session gsettings set org.gnome.login-screen.gaze enable-face-authentication false
```

## Verify GNOME flow

- Lock screen, then try unlock with face.
- If login face auth is enabled, test a full logout/login cycle.
