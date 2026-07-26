# COSMIC (lock screen and login)

Gaze integrates with [COSMIC](https://system76.com/cosmic/), the desktop environment used by Pop!_OS, through [cosmic-greeter](https://github.com/pop-os/cosmic-greeter). One integration covers both places COSMIC asks for a password:

- the **lock screen**, where `cosmic-greeter` runs as your user
- the **login screen**, where greetd runs `cosmic-greeter` as its own user

Both authenticate through the PAM service named `cosmic-greeter`, which is why enabling Gaze once covers both.

## Install

The one-line installer auto-installs `gaze-cosmic` and enables it when it detects a COSMIC session or an installed `cosmic-greeter`.

Manual install:

::: code-group

```bash [Debian/Ubuntu/Pop!_OS]
sudo apt-get install gaze-cosmic
```

```bash [Fedora and compatible]
sudo dnf install gaze-cosmic
```

```bash [Arch]
yay -S gaze-cosmic-bin
```

:::

## Enable and disable

The package wires itself in on install. To check, enable, or remove the wiring at any time:

```bash
gaze-cosmic-pam status
sudo gaze-cosmic-pam enable
sudo gaze-cosmic-pam disable
```

Nothing needs restarting. The next lock or login reads the updated PAM stack. Uninstalling `gaze-cosmic` removes the wiring too.

## What the wiring changes

Unlike hyprlock, `cosmic-greeter` hardcodes its PAM service name, so Gaze cannot ship a separate service file for you to opt into. Instead `gaze-cosmic-pam` inserts a marked block into the distro's `/etc/pam.d/cosmic-greeter`:

```ini
# BEGIN gaze (managed by gaze-cosmic; remove with `gaze-cosmic-pam disable`)
auth       [success=done default=ignore]                pam_gaze.so
# END gaze
```

The block goes above the first module that would ask for a password, and below gate modules such as `pam_nologin.so` and `pam_succeed_if.so`, so a face match cannot skip those checks. `disable` removes exactly that block and leaves anything you added by hand alone. The untouched original is kept at `/etc/gaze/cosmic-greeter.pam.orig`.

## How it works

1. You lock the session (or reach the login screen) and `cosmic-greeter` starts PAM with the service name `cosmic-greeter`
2. `pam_gaze.so` claims the camera through the `gazed` DBus service and runs face verification
3. On a match → `PAM_SUCCESS`, and `[success=done]` ends the auth stack: the session unlocks or logs in
4. On no enrolled faces or an unavailable daemon → `PAM_IGNORE`, and COSMIC shows its password field
5. On no match, darkness, or a timeout → the stack falls through to the password field

While face authentication runs, COSMIC replaces its password field with the message `Please look at the camera. The password field returns if face authentication fails.` cosmic-greeter displays one PAM message at a time, so the field is genuinely unavailable during that window (the same is true of fingerprint readers there). It is short in practice: the daemon gives up after about a second when the scene is too dark and about five seconds when it sees no face at all, with a hard ceiling of twelve seconds. Covering the camera is the quickest way to get the password field back.

Each failed attempt restarts the whole PAM conversation, so a wrong password sends you through the camera window again before the password field comes back.

No `gazed` configuration changes are required. The DBus policy already lets an unprivileged session claim the camera and verify against its own enrolled templates.

## Require Confirmation is not available on COSMIC

If you enable `require_confirmation` (see [Configuration](/guide/configuration)), a face match on COSMIC is *not* accepted: Gaze falls back to the password, exactly as it does on the KDE lock screen and `hyprlock`. Gaze never treats an unseen request as confirmed, and COSMIC has nowhere to show one:

- `cosmic-greeter` displays one PAM message at a time, and once you submit an answer it disables its input until the whole authentication attempt finishes. Spending that single answer on a confirmation is only safe if nothing later in the stack asks another question, which Gaze cannot guarantee: a second-factor module, for instance, would leave you looking at a field you cannot type into.
- `cosmic-osd`, the COSMIC polkit dialog, does not display PAM info messages at all. This is also why privileged prompts there show an unlabelled password field for the second or two that face verification takes: Gaze's status messages never reach the dialog. Typing your password while it runs is fine, and it is used as soon as the camera gives up.

So on COSMIC, leave `require_confirmation = false` (the default) if you want face unlock to work on the lock screen, login screen, and polkit prompts.

## Simultaneous mode is not supported on COSMIC

The `pam_gaze_grosshack.so` module (`gaze-simultaneous`, used for typing a password while the camera runs) needs a password prompt to stay pending while face verification happens in the background. `cosmic-greeter` shows one prompt at a time and has no way to withdraw one it has already displayed, so Gaze wires COSMIC with the sequential module only.

For the same reason, keep the **sequential** profile (`gaze`) selected for your shared auth stack on COSMIC rather than `gaze-simultaneous`, since the COSMIC lock screen also runs whatever your system stack contains:

::: code-group

```bash [Debian/Ubuntu/Pop!_OS]
sudo pam-auth-update  # select "Gaze Face Authentication (Sequential)"
```

```bash [Fedora and compatible]
sudo authselect select gaze with-silent-lastlog --force
```

:::

## Prerequisites

- `gazed` daemon running (`systemctl status gazed`)
- At least one enrolled face: `gaze add-face default`
- Working camera (test with `gaze auth`)
- For the login screen: a PipeWire runtime in the greeter's own session. `gazed` binds capture to the active seat session, and refuses the claim when the greeter has no `/run/user/<greeter-uid>/pipewire-0`, logging `refusing face auth: no camera belongs to the target user's session`. greetd starts a systemd user session for `cosmic-greeter`, so this is normally present.
- Also for the login screen: a camera source that does not depend on a PipeWire camera portal. Greeters often do not hand one out, so prefer `cameras.rgb = "usb:VVVV:PPPP"` or a `/dev/video<n>` node over `primary`; set it with `gaze config`.

## Verify

```bash
gaze doctor
```

From a COSMIC session, `doctor` reports a **COSMIC lock screen** check that shows whether `/etc/pam.d/cosmic-greeter` authenticates through Gaze, and a **COSMIC simultaneous mode** warning if the stack it includes pulls in `pam_gaze_grosshack.so`.

## Troubleshooting

- **Password field appears immediately, camera never runs**: the wiring is missing. Run `gaze-cosmic-pam status`, then `sudo gaze-cosmic-pam enable`.
- **Falls back to the password every time**: the daemon may not be running, or no faces are enrolled for that user. Check `systemctl status gazed` and `gaze list-faces`.
- **Login screen falls back but the lock screen works**: check `journalctl -u gazed` for `no camera belongs to the target user's session` (the greeter has no PipeWire runtime) or for capture errors against `primary` (switch to a `usb:` or `/dev/video<n>` source), as described under Prerequisites.
- **Camera busy**: another Gaze client (the GUI, another session) holds the camera. Close it and retry.
- **Keyring stays locked after a face login**: `[success=done]` ends the auth stack at the match, so `pam_gnome_keyring.so` never sees a password to unlock the login keyring with. This is the same trade-off Gaze makes at GDM; unlock the keyring with its own prompt, or use the password login when you need it unlocked.
- **A distro upgrade replaced the PAM file**: re-run `sudo gaze-cosmic-pam enable`.
