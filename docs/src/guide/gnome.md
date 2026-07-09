# GNOME Extension

Gaze lock screen and GDM integration are GNOME-specific and require the `gaze-gnome-extension` package. The one-line installer tries to enable lock screen face unlock for the current GNOME user. Manual package installs only install the extension files.

This extension starts the `gdm-face` PAM service inside GNOME Shell authentication flows.

You do not need to enable this extension for the CLI, the GUI, or normal PAM prompts such as `sudo`. Leave it disabled on non-GNOME desktops.

> [!IMPORTANT]
> If you enable `require_confirmation = true` in `/etc/gaze/config.toml`, this GNOME Shell Extension **must** be enabled for face-authorization confirmation to function inside GNOME's graphical PolKit prompts and on the lock screen / GDM login screen.
> 
> **Why this is required:** Standard GNOME PolKit prompt windows do not natively allow clicking "Authenticate" with an empty or blank password field. The GNOME Shell Extension solves this by dynamically intercepting Gaze's confirmation signals, automatically hiding the password entry, displaying `"Face Verified. Click Authenticate to confirm."`, and enabling the native "Authenticate" button. On the lock screen, GNOME Shell drops prompts from background PAM services such as `gdm-face`, so the extension routes Gaze's confirmation request to the unlock prompt as `"Face Verified. Press Enter to confirm."`; pressing Enter with an empty password field confirms.
> 
> If the extension is **inactive/disabled** under GNOME while `require_confirmation = true` is set, Gaze's PAM modules will **safely bypass confirmation** (returning success instantly upon face match) to prevent empty input hangs and user lockouts.

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

## Create a face profile

Open the Gaze extension settings from GNOME Extensions or Extension Manager, then use **Face profiles** to create or refine a profile for your current user. The profile name defaults to `default`, matching the CLI quick-start flow.

Keep the settings window open while enrollment is running. Follow the camera prompts until the profile is saved, or press **Cancel** to stop enrollment.

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

At the GDM login screen, Gaze still matches against the selected user's enrolled faces, but captures through the greeter's PipeWire camera session: while the greeter owns the seat it also holds the camera device access, so even a user session lingering in the background (after a logout or user switch) can no longer capture.

## Disable face at GDM login

Flip the **Enable face auth at GDM login** switch back off in the extension preferences, or remove the override manually:

```bash
sudo rm -f /etc/dconf/db/gdm.d/99-gaze*
sudo dconf update
```

## Verify GNOME flow

- Lock screen, then try unlock with face.
- If login face auth is enabled, test a full logout/login cycle.
