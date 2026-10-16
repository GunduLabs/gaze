<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# CLI Guide

The `gaze` command lets you enroll and manage faces, test authentication,
and check your installation.

All commands talk to the running `gazed` daemon over DBus.

## Commands that need privileges

An enrolled face is a login credential, so creating, changing, or deleting one
requires root privileges, even for your own account. You do not need to run
`sudo` yourself: commands such as `gaze add-face`, `gaze refine-face`,
`gaze remove-face`, `gaze rename-face`, `gaze clear-user`, `gaze config`, and
`gaze keyring` request elevated privileges and prompt for your password when
needed. They still act on the account that invoked them, not on `root`, so
`gaze add-face default` enrolls a face for you. To manage root's enrollment,
pass `-u root`.

`gaze auth`, `gaze list-faces`, `gaze doctor`, `gaze config --show`, and `gaze config get` are read-only and stay unprivileged. `gaze config set` asks for privileges like the wizard does.

The `gazed` daemon enforces this independently of the CLI: a non-root DBus caller must pass a polkit check for `com.gundulabs.gaze.manage-faces` or `com.gundulabs.gaze.manage-config`. That is how the [GUI](/guide/gui) authorizes the same operations, and it means calling the DBus methods directly does not bypass the requirement. While one of these prompts is open, `gazed` refuses face authentication for the polkit dialog, so changing Gaze's faces or settings always takes a password.

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
- GNOME, KDE, or hyprlock integration when running those desktops (on KDE it reports which biometric slot runs Gaze, and whether the login greeter scans before you type or on submit)
- TPM availability when encrypted template storage is enabled
- GNOME Keyring unlock: whether it is on, whether its prerequisites are met, whether `/etc/pam.d/gdm-face` still passes the token, and whether the user has a credential enrolled

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

For scripts, `gaze doctor --json` prints every check as JSON instead of the usual list.

When filing a bug, run:

```bash
gaze doctor --report
```

It writes a `gaze-report-<time>.txt` file to the current directory (or the temporary directory if that is not writable). The file holds the doctor results, Gaze version, OS release, kernel, desktop session, installed Gaze packages, every PAM line that mentions Gaze, the `gazed` service status and its last 200 journal lines, and `/etc/gaze/config.toml`. Your user name and host name are replaced with `<user>` and `<host>`. Read it before attaching it to an issue.

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

## Enroll a new face

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

Use this if recognition is inconsistent in dim light or side angles. This also captures and adds missing camera spectra (e.g. adding IR captures to an RGB-only face if an IR camera was configured after initial enrollment).

## List, rename, and remove

```bash
gaze list-faces
gaze rename-face <old> <new>
gaze remove-face <name>
```

`gaze list-faces` prints all faces enrolled for the user, showing how many template captures each face has, and displaying `[RGB]` and `[IR]` badges for the camera spectra enrolled in that profile:

| Badge | Meaning |
|---|---|
| Green | The profile has captures for that spectrum. |
| Amber | A camera is configured for that spectrum, but this profile has no captures from it. Run `gaze refine-face <name>`. |
| Grey | No camera is configured for that spectrum, so there is nothing to enrol. |

An RGB-only machine therefore shows a green `[RGB]` and a grey `[IR]`, not a failure. `gaze list-faces --json` prints the same list as JSON.

`gaze remove-face` and `gaze rename-face` exit with status `1` when the face does not exist.

## Delete all faces for current user

```bash
gaze clear-user
```

This permanently removes all enrolled faces and any stored GNOME Keyring or KWallet credential for the current user. It asks for confirmation first; pass `--yes` to skip the prompt in scripts. Without a terminal and without `--yes`, it refuses.

## Check or clear a duress lockout

When [duress detection](./configuration.md#duress-signal) locks face
authentication, a password login re-enables it. To check or clear it by hand:

```bash
gaze duress          # show whether face auth is locked
gaze duress --clear  # re-enable face auth
```

Clearing a lockout manually requires fresh administrator authentication, even
for your own account. Root PAM callers clear it after a successful non-face login.

## Unlock the GNOME Keyring after a GDM or greetd face login

A face login supplies no password, so the GNOME login keyring normally stays
locked. `gaze keyring` stores the account password in a root-only TPM-protected
record that `pam_gaze.so` replays to `pam_gnome_keyring` after a successful face
and liveness check:

```bash
gaze keyring                    # enroll or replace the credential
gaze keyring --forget           # remove the stored GNOME credential
gaze keyring --kwallet          # enroll or replace the KDE wallet credential
gaze keyring --kwallet --forget # remove only the KDE wallet credential
sudo gaze keyring --user alice  # act on another account
```

For KDE, use `--kwallet` with `storage.unlock_kwallet = true` and the
[managed KDE login PAM hooks](/guide/kde#optional-tpm-backed-kwallet-unlock).
The two backends have separate credentials; `gaze clear-user` removes both.

GNOME enrollment needs `storage.unlock_gnome_keyring = true`, which in turn needs
`storage.encrypt_templates = true` and `liveness.enabled = true`. `gaze
clear-user` removes the record along with the rest of the user's data.

Read [What this changes about your security](/guide/gnome#what-this-changes-about-your-security)
before enabling it: the stored password is recoverable by root on this machine.

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

Prints every setting as TOML, in the same layout as `/etc/gaze/config.toml`, followed by comments with the values that presets resolve to (detector, recognizer, thresholds, hybrid policy, parallel capture, and start delay scope).

To read or change one setting without the wizard:

```bash
gaze config get security.level
gaze config set liveness.enabled true
gaze config set cameras.dark_luma_threshold 40
```

`set` checks the value against the setting's type and the same validation the daemon uses, then applies it through `gazed`.

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
