# GNOME Extension

Gaze lock screen and GDM integration are GNOME-specific and require the `gaze-gnome-extension` package. The one-line installer tries to enable lock screen face unlock for the current GNOME user. Manual package installs only install the extension files.

This extension starts the `gdm-face` PAM service inside GNOME Shell authentication flows.

You do not need to enable this extension for the CLI, the GUI, or normal PAM prompts such as `sudo`. Leave it disabled on non-GNOME desktops.

## Should I enable it?

Enable it if you use GNOME and want face unlock from the lock screen.

Do not enable it if you only want CLI/GUI enrollment, normal PAM authentication, or you are not using GNOME.

## Enable the extension

If the package is installed but the extension is not enabled yet, first reboot so GNOME Shell scans the newly installed extension. Then, from your GNOME session:

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

`gnome-extensions enable` will report `Extension "gaze@gundulabs.com" does not exist` if you run it before rebooting. Shell only scans extension directories at session start, so running the command immediately after install (without a session restart) always fails. If you cannot reboot yet, the equivalent dconf write works at any time and takes effect on the next login:

```bash
gsettings set org.gnome.shell enabled-extensions \
  "$(gsettings get org.gnome.shell enabled-extensions | sed "s/]\$/, 'gaze@gundulabs.com']/; s/^@as \[\]\$/['gaze@gundulabs.com']/")"
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

## Login warning (GNOME keyring)

GDM loads the extension from package defaults, but face authentication for the GDM login screen is disabled by default.

This is mostly about GNOME keyring behavior. GNOME keyring is normally unlocked by your login password. If you log in with face only, that password is never entered, so the keyring may stay locked.

When that happens, apps that read saved secrets (browser credentials, git credentials, Wi-Fi secrets, chat clients, etc.) can keep prompting for a keyring password until you unlock it manually.

## Optional: enable face at GDM login

The easiest way is the **Enable face auth at GDM login** switch in the Gaze extension preferences (Extensions app → Gaze → cog icon). Toggling it triggers a polkit prompt, then the daemon writes `/etc/dconf/db/gdm.d/99-gaze` and runs `dconf update` for you.

Reboot to apply. Restarting GDM also works, but it immediately logs out active desktop sessions.

```bash
sudo reboot
```

### Manual alternative

If you prefer to do it from a terminal:

```bash
sudo tee /etc/dconf/db/gdm.d/99-gaze >/dev/null <<'EOF'
[org/gnome/shell/extensions/gaze]
enable-face-authentication=true
EOF
sudo dconf update
```

At the GDM login screen, the selected user's desktop session may not exist yet. Gaze still matches against that user's enrolled faces, but uses the active greeter PipeWire camera session when needed.

## Disable face at GDM login

Flip the **Enable face auth at GDM login** switch back off in the extension preferences, or remove the override manually:

```bash
sudo rm -f /etc/dconf/db/gdm.d/99-gaze*
sudo dconf update
```

## Verify GNOME flow

- Lock screen, then try unlock with face.
- If login face auth is enabled, test a full logout/login cycle.
