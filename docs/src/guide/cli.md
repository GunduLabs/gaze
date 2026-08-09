<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# CLI Guide

Use the `gaze` command for enrollment, testing, and managing face profiles.

All commands talk to the running `gazed` daemon over DBus.

## Commands that need privileges

An enrolled face is a login credential, so creating, changing, or deleting one requires root, even on your own account. You never type `sudo` yourself: `gaze add-face`, `gaze refine-face`, `gaze remove-face`, `gaze rename-face`, `gaze clear-user`, and `gaze config` re-run themselves through `sudo` and prompt for your password. They still act on the account that invoked them, not on `root`, so `gaze add-face default` enrolls a face for you; pass `-u root` if you really want root's own enrollment.

`gaze auth`, `gaze list-faces`, `gaze doctor`, and `gaze config --show` are read-only and stay unprivileged.

The `gazed` daemon enforces this independently of the CLI: a non-root DBus caller must pass a polkit check for `com.gundulabs.gaze.manage-faces` or `com.gundulabs.gaze.manage-config`. That is how the [GUI](/guide/gui) authorizes the same operations, and it means calling the DBus methods directly does not bypass the requirement.

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

To measure model inference speed on the current hardware:

```bash
gaze doctor --benchmark
```

This asks the running `gazed` daemon to run each loaded model (detector, RGB/IR recognizers, and liveness if enabled) a few times on synthetic input and reports average, p95, and minimum latency plus FPS for each. It does not need a face in frame and does not touch the camera.

## Authenticate

```bash
gaze auth
```

Useful options:

```bash
gaze auth -v          # show detailed authentication metrics (short form)
gaze auth --verbose   # same
gaze auth -s          # run silently and report the result via exit code (short form)
gaze auth --silent    # same
```

`--verbose` and `--silent` are mutually exclusive.

Result meanings:

- `✓ Authenticated as: <face> (XX.X%, XXXms)`: pass: matched face name, score percentage, and elapsed time
- `✗ Authentication failed (XXXms)`: no face passed the current threshold or liveness check

With `--verbose`, a per-face table is printed before the result showing the per-spectrum (RGB and IR) similarity score, match percentage, and pass/fail for each enrolled face.

### Exit codes

`gaze auth` reports its outcome through the exit status, so it can be used directly in scripts and `if` conditions:

| Code | Meaning |
| --- | --- |
| `0` | A face matched and the user was authenticated |
| `1` | Authentication failed, no faces are enrolled, or the daemon could not be reached |
| `130` | Cancelled from the terminal UI |

This applies with and without `--silent`. Earlier releases exited `0` on a failed authentication, so scripts that only checked the exit status need to be re-checked.

### Silent mode

`gaze auth --silent` skips the terminal UI entirely and writes nothing to stdout or stderr; the exit code is the only result. Use it in PAM helpers, scripts, and other headless contexts where no TTY is attached:

```bash
if gaze auth --silent; then
  echo "welcome back"
fi
```

Because silent mode is non-interactive, it cannot be cancelled from the terminal UI and it does not start a polkit agent, so authenticating another user with `--user` fails instead of prompting. Silent mode has no timeout of its own, wrap it in `timeout(1)` if the caller needs a deadline:

```bash
timeout 15 gaze auth --silent
```

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

Prints all current config values without opening the editor: inference execution provider and device, security level, detector and recognizer model, RGB and IR thresholds, hybrid combining policy (both the raw value and what it resolves to), camera sources, emitter state, dark-frame threshold, auth behavior, enrollment limit and minimum face-size ratio, liveness settings, and whether template encryption is on.

## Shell completions

`gaze` generates its own completions at runtime, including the names of your
enrolled faces for commands that take one. Add the line for your shell to its
startup file:

::: code-group

```bash [bash]
echo 'source <(COMPLETE=bash gaze)' >> ~/.bashrc
```

```zsh [zsh]
echo 'source <(COMPLETE=zsh gaze)' >> ~/.zshrc
```

```fish [fish]
echo 'COMPLETE=fish gaze | source' >> ~/.config/fish/config.fish
```

:::

Start a new shell to pick them up. Nothing is installed to disk, so there is no
completion file to keep in sync when you enroll or rename a face.

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
