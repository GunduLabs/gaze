<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# GNOME Extension

Gaze lock screen and GDM integration are GNOME-specific and require the `gaze-gnome-extension` package. The one-line installer tries to enable lock screen face unlock for the current GNOME user. Manual package installs only install the extension files. On openSUSE Tumbleweed, install the extension with `sudo zypper install gaze-gnome-extension` before enabling it.

This extension starts the `gdm-face` PAM service inside GNOME Shell authentication flows.

You do not need to enable this extension for the CLI, the GUI, or normal PAM prompts such as `sudo`. Leave it disabled on non-GNOME desktops.

> [!IMPORTANT]
> If you enable `require_confirmation_lock_screen = true` or `require_confirmation_elevation = true` in `/etc/gaze/config.toml`, this GNOME Shell Extension **must** be enabled for face-authorization confirmation to function inside GNOME's graphical PolKit prompts and on the lock screen / GDM login screen.
> 
> **Why this is required:** Standard GNOME PolKit prompt windows and lock screen prompts do not natively allow clicking confirmation buttons with an empty or blank password field. The GNOME Shell Extension solves this by dynamically intercepting Gaze's confirmation signals, automatically hiding the password entry, and focusing the confirmation button (the native "Authenticate" button in PolKit and a dedicated "Confirm Face Unlock" button on the lock screen and GDM login dialog).
> 
> If the extension is **inactive/disabled** under GNOME while either toggle is set, Gaze's PAM modules will **safely bypass confirmation** (returning success instantly upon face match) to prevent empty input hangs and user lockouts.

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

### The extension disappears again after a logout

Adding the UUID by hand names an extension the running GNOME Shell has never scanned. Shell drops UUIDs it does not recognise the next time it rewrites `enabled-extensions`, which it does when the session ends or when you toggle any other extension. So the setting can look correct right after install and be gone after the first logout, without anything having failed.

Reboot rather than log out after installing, so Shell scans the extension before it rewrites the list.

If it has already vanished, run the two commands under [Enable the extension](#enable-the-extension) from a GNOME session that started **after** the package was installed. `gaze doctor` reports this case as `GNOME extension: installed, but not enabled for the current user` and prints the same steps.

The one-line installer also leaves a one-shot autostart entry, `~/.config/autostart/gaze-gnome-enable.desktop`, that re-applies the enable at your next GNOME login and then deletes itself along with its helper at `~/.local/share/gaze/gnome-enable.sh`. Both are safe to delete by hand if you would rather do it yourself.

## Open the extension preferences

```bash
gnome-extensions prefs gaze@gundulabs.com
```

Or open the **Extensions** app (Extension Manager works too), find **Gaze**, and open its settings from the row.

The window has a single **Behavior** page with two groups:

| Group | Contains |
|---|---|
| **Face authentication** | `Enable face authentication (lock screen)`, `Face retry mode`, `Maximum face tries`. Applies to this session's lock screen only. |
| **GDM login screen** | `Enable face auth at GDM login`. Applies to the login screen and asks for admin authorization. |

## Retry behavior

The extension decides how many times face authentication is retried within one
authentication cycle. Both settings live under **Behavior → Face authentication**
in the extension preferences, and both are per dconf profile, so GDM and your
desktop session can differ.

| Setting | dconf key | Values | Default |
|---|---|---|---|
| Face retry mode | `face-retry-mode` | `disabled`, `fixed`, `infinite` | `fixed` |
| Maximum face tries | `max-face-tries` | 2 to 20 | 3 |

- `disabled`: one attempt. After it fails, face auth stops for that cycle and you
  finish with your password.
- `fixed`: retries until `max-face-tries` failures, then stops for that cycle.
- `infinite`: keeps retrying for as long as the prompt is open. The password entry
  stays usable throughout.

`max-face-tries` only applies in `fixed` mode, and the extension clamps it to a
minimum of 2 even if dconf holds a lower value.

From a terminal:

```bash
gsettings set org.gnome.shell.extensions.gaze face-retry-mode infinite
gsettings set org.gnome.shell.extensions.gaze max-face-tries 5
```

To set these for the GDM login screen, write them into the same
`/etc/dconf/db/gdm.d/99-gaze` override described below and run `sudo dconf update`.

## Create a face profile

Enrollment does not live in the extension preferences. Use the Gaze settings app or the CLI:

```bash
gaze-gui             # Faces list, press + to enroll
gaze add-face default
```

The profile name defaults to `default`, matching the CLI quick-start flow. Follow the camera prompts until the profile is saved.

## Login warning (GNOME keyring)

GDM loads the extension from package defaults, but face authentication for the GDM login screen is disabled by default.

This is mostly about GNOME keyring behavior. GNOME keyring is normally unlocked by your login password. If you log in with face only, that password is never entered, so the keyring may stay locked.

When that happens, apps that read saved secrets (browser credentials, git credentials, Wi-Fi secrets, chat clients, etc.) can keep prompting for a keyring password until you unlock it manually.

## Optional: enable face at GDM login

The easiest way is the **Enable face auth at GDM login** switch, under **Behavior → GDM login screen** in the [extension preferences](#open-the-extension-preferences). Toggling it triggers a polkit prompt, then the daemon writes `/etc/dconf/db/gdm.d/99-gaze` and runs `dconf update` for you.

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

Flip the **Enable face auth at GDM login** switch back off under **Behavior → GDM login screen**, or remove the override manually:

```bash
sudo rm -f /etc/dconf/db/gdm.d/99-gaze*
sudo dconf update
```

## Verify GNOME flow

- Lock screen, then try unlock with face.
- If login face auth is enabled, test a full logout/login cycle.
