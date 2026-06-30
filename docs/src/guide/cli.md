# CLI Guide

Use the `gaze` command for enrollment, testing, and managing face profiles.

All commands talk to the running `gazed` daemon over DBus.

## Most common workflow

```bash
gaze add-face default
gaze auth --verbose
gaze refine-face default
gaze list-faces
gaze doctor
```

## Diagnose the installation

Run the read-only diagnostic command from your local graphical session:

```bash
gaze doctor
```

It checks:

- CPU and systemd service compatibility
- `/etc/gaze/config.toml` parsing, permissions, and unsafe values
- daemon and system DBus responsiveness
- access to the current PipeWire session and visibility of configured RGB/IR cameras
- face enrollment and RGB/IR capture coverage for the current user
- PAM module installation, permissions, and active PAM stack references
- GNOME or hyprlock integration when running those desktops
- TPM availability when encrypted template storage is enabled

Every warning or error includes a suggested next step. Errors that can prevent Gaze from working make the command exit with status `1`; warnings are advisory and leave the exit status at `0`.

To inspect enrollment for another user (subject to the normal DBus authorization rules):

```bash
gaze doctor --user alice
```

The camera checks enumerate devices but do not capture frames. Use `gaze auth` when you need an end-to-end camera and recognition test.

## Authenticate

```bash
gaze auth
```

Useful options:

```bash
gaze auth -v          # show detailed authentication metrics (short form)
gaze auth --verbose   # same
```

Result meanings:

- `✓ Authenticated as: <face> (XX.X%, XXXms)`: pass - matched face name, score percentage, and elapsed time
- `✗ Authentication failed (XXXms)`: no face passed the current threshold or liveness check

With `--verbose`, a per-face table is printed before the result showing similarity score, match percentage, passed/failed, and template count for each enrolled face.

## Enroll a new face profile

```bash
gaze add-face <name>
```

Examples:

```bash
gaze add-face default
gaze add-face glasses
```

Use separate profiles when your appearance changes often.

## Improve a profile

```bash
gaze refine-face <name>
```

Use this if recognition is inconsistent in dim light or side angles. This also captures and adds missing camera spectra (e.g. adding IR captures to an RGB-only face profile if an IR camera was configured after initial enrollment).

## List, rename, and remove

```bash
gaze list-faces
gaze rename-face <old> <new>
gaze remove-face <name>
```

`gaze list-faces` prints all enrolled face profiles for the user, showing how many template captures each face has, and displaying `[RGB]` and `[IR]` status badges in green/red to indicate which camera spectrums are enrolled for that profile.

## Delete all faces for current user

```bash
gaze clear-user
```

This is destructive.

## Uninstall Gaze completely

```bash
gaze uninstall              # interactive
gaze uninstall --yes        # skip confirmation
gaze uninstall --keep-data  # preserve enrolled faces in /var/lib/gaze
gaze uninstall --dry-run    # preview the plan, run nothing
```

Removes the installed packages, repository config, GNOME/GDM lock and login settings, PAM/authselect integration, SELinux policy, the model cache (`/var/cache/gaze`), the system config (`/etc/gaze`), and (unless `--keep-data` is set) enrolled face data (`/var/lib/gaze`). Each step is best-effort and uses `sudo`, so you'll be prompted for your password.

See the [uninstallation guide](/guide/uninstallation) if you'd rather run the steps manually.

## Interactive configuration

Use the interactive wizard to edit daemon config through DBus:

```bash
gaze config
```

Show-only mode:

```bash
gaze config --show
```

Prints all current config values (security level, detector and recognizer model, threshold, camera sources, emitter state, dark-frame threshold, auth behavior, hybrid combining policy, enrollment limit, and liveness settings) without opening the editor.

## Manage another user

Most commands support `-u`:

```bash
gaze list-faces -u alice
gaze add-face work -u alice
```

## Troubleshooting commands

```bash
gaze doctor
systemctl status gazed
journalctl -u gazed -n 100 --no-pager
gaze auth --verbose
```

If you need help diagnosing failures, see the [troubleshooting guide](/guide/troubleshooting).
