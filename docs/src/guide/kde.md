# KDE Plasma

On the KDE Plasma **lock screen**, the `gaze-kde` package makes face unlock start
on its own, with no key press. The **login greeter** is a separate program with a
different limitation, and is off by default; see [Login greeter](#login-greeter).

The one-line installer installs `gaze-kde` when it detects a KDE Plasma session,
so these steps are only needed after a manual install.

## Install

::: code-group

```bash [Debian/Ubuntu]
sudo apt-get install gaze-kde
```

```bash [Fedora and compatible]
sudo dnf install gaze-kde
```

```bash [Arch]
yay -S gaze-kde-bin
```

:::

`gaze-kde` wires up the lock screen and adds a System Settings entry. The one-line
installer installs it when it detects a KDE Plasma session.

Lock your screen and look at the camera. Face auth runs while the password field
waits; whichever succeeds first wins. `gaze doctor` reports whether it is wired
up under "KDE lock screen".

## System Settings

`gaze-kde` registers a **Face Unlock** entry in System Settings that opens the Gaze
app, so face setup is where you would look for it rather than only in the
application launcher. Selecting it launches `gaze-gui`, which manages enrolled
faces and every Gaze setting.

That app is the GTK one, so it does not match Plasma's styling. The trade-off is
deliberate: one UI that always has the full feature set beats a Plasma-native page
covering only part of it. Install `gaze-gui` (the installer does) for the entry to
have something to open.

## How the lock screen works

KScreenLocker's greeter starts three PAM services at once for a single unlock:
the interactive `kde` service for the password field, plus two *noninteractive*
slots that run up front for biometrics, `kde-fingerprint` and `kde-smartcard`.
That is the same slot a fingerprint reader uses, and it is why hands-free face
unlock is possible here at all.

`gaze-kde` puts Gaze in the `kde-fingerprint` slot:

```
auth        [success=done default=ignore]                pam_gaze.so
```

1. The greeter calls that service as soon as the lock screen appears, before you
   touch anything.
2. Gaze claims the camera through `gazed` and verifies your face.
3. On a match, `success=done` ends the stack and the screen unlocks.
4. On anything else, `default=ignore` falls through to the rest of the stack — a
   fingerprint reader, if you have one — and the password field carries on
   waiting the whole time.

Gaze is inserted **ahead** of any fingerprint reader in that file, because
`pam_fprintd` blocks waiting for a swipe; behind it, face auth would never get a
turn. With both installed you get roughly twelve seconds of face unlock, then the
reader.

### One attempt per lock

The greeter gives a noninteractive slot a single authentication per lock: it
deliberately ignores biometric failures and only re-arms everything when a
*password* attempt fails. Because the daemon stops looking a few seconds after it
loses sight of a face, Gaze keeps retrying internally for its whole budget rather
than giving up on the first empty frame. Waking the screen and looking up a
moment later still unlocks.

### Status messages

The lock screen renders PAM *error* messages from a biometric slot but discards
informational ones, so Gaze sends anything you need to read ("Face not
recognized", "Too dark for face authentication") as an error message. It appears
in place of the "(or scan your fingerprint on the reader)" hint.

### Only one Gaze per lock screen

On Fedora and Debian, Gaze installs into the shared authentication stack
(`password-auth`, `common-auth`) that `/etc/pam.d/kde` also includes. Once
`kde-fingerprint` runs Gaze, the module stands down in the `kde` and
`kde-smartcard` services so a single lock screen does not run face auth two or
three times over and have those clients fight over one camera. Removing
`gaze-kde` restores the previous behaviour automatically — the module decides by
reading `/etc/pam.d/kde-fingerprint`, not from a build flag.

## require_confirmation

With `require_confirmation = true`, the face match unlocks the KDE lock screen on
its own. There is no way to present a confirmation there: the greeter never
delivers a response to a noninteractive slot, so asking would hang that slot for
the rest of the lock rather than ask anybody anything. Denying the match instead
would just mean no face unlock at all on KDE.

If you want a real confirmation step on KDE, use `pam-gaze-grosshack` on a
surface that can show a dialog, such as polkit prompts. It refuses to run in the
lock screen's biometric slot for the reason above.

## Login greeter

The login greeter (Plasma Login Manager, or SDDM) is a different program from the
lock screen, and upstream gives it **no** up-front biometric slot at all — there
is no `plasmalogin-fingerprint` equivalent. It runs its PAM stack when you submit
the login form, so face auth there behaves like a fingerprint reader on SDDM:
press Enter with the password field empty and look at the camera.

It is off by default. To turn it on:

```bash
sudo gaze-kde-pam enable-login
```

That inserts Gaze into `/etc/pam.d/plasmalogin` and `/etc/pam.d/sddm`, whichever
exist, after the stack's gate modules (`pam_nologin`, the `user != root` check,
`pam_selinux_permit`) so a face match cannot skip them. Turn it back off with
`sudo gaze-kde-pam disable-login`.

On Fedora and Debian it will report that the stack **already reaches Gaze** and
change nothing. That is correct: those login stacks include the shared
authentication stack Gaze installs into (`password-auth`, `common-auth`), so face
auth already runs at the greeter on submit. Inserting a second line would make a
failed scan run the camera twice over before the password prompt appeared. In
practice `enable-login` only has work to do on Arch, where Gaze is wired into
`sudo` and `polkit-1` alone.

::: warning KWallet asks for its password after a face login
KWallet unlocks itself at login by reusing the password you typed. After a face
login there is no password to reuse, so KWallet prompts you for one once, in the
session. `success=done` keeps that prompt out of the greeter itself, where it
would otherwise appear as a second password box.

Nothing to do on the lock screen: KWallet only unlocks at login.
:::

## Managing it by hand

`gaze-kde-pam` is the same helper the package's install and removal scripts call:

```bash
sudo gaze-kde-pam enable         # lock screen
sudo gaze-kde-pam disable
sudo gaze-kde-pam enable-login   # login greeter (opt-in)
sudo gaze-kde-pam disable-login
gaze-kde-pam status
```

It edits `/etc/pam.d/kde-fingerprint` in place inside a marked block, because on
most distributions that file belongs to `plasma-workspace` and overwriting it
would drop your fingerprint reader. Where a distribution ships no such file at
all, `enable` creates it and `disable` removes it again. A `pam_gaze` line you
added yourself, outside the marked block, is left alone.

## Prerequisites

- `gazed` running (`systemctl status gazed`)
- At least one enrolled face: `gaze add-face default`
- A working camera (test with `gaze auth`)

## Cameras at the login greeter

The lock screen runs inside your session, so the camera works there exactly as it
does for `sudo`. The login greeter does not: SDDM's and Plasma Login Manager's
greeter accounts have no user session and therefore no PipeWire, unlike GDM's.
Gaze captures the seat's V4L2 device directly in that case, so `rgb = "primary"`
still works at the greeter.

If your camera is not picked up there, name it explicitly so resolution never
depends on a session:

```toml
[cameras]
rgb = "usb:046d:085e"   # hex VID:PID from `lsusb`
```

See [Configuration](/guide/configuration) for the full set of camera options.

## Disable

::: code-group

```bash [Debian/Ubuntu]
sudo apt-get remove gaze-kde
```

```bash [Fedora and compatible]
sudo dnf remove gaze-kde
```

```bash [Arch]
yay -R gaze-kde-bin
```

:::

## Troubleshooting

- **Face unlock still waits for a password submit.** `kde-fingerprint` is not
  running Gaze. Check the "KDE lock screen" line in `gaze doctor`, then run
  `sudo gaze-kde-pam enable`.
- **`gaze doctor` warns that `kde-fingerprint` runs `pam_gaze_grosshack.so`.**
  That module waits for a password prompt the greeter can never answer. Use the
  plain `pam_gaze.so` line there instead; reinstalling `gaze-kde` fixes it.
- **Falls back to the password every time.** Check `systemctl status gazed` and
  `gaze list-faces` — most often the daemon is not running or the current user
  has no enrolled face.
- **Camera busy.** Another Gaze client (the GUI, or the GNOME extension on a
  mixed install) holds the camera. Close it and retry.
- **The Face Unlock entry is missing from System Settings.** Restart System
  Settings; it only scans for modules at startup. If it still does not appear,
  check that `/usr/share/plasma/systemsettings/externalmodules/gaze-face-unlock.desktop`
  exists and that `gaze-gui` is installed.
- **The camera never comes on at the login greeter.** That path is opt-in; run
  `sudo gaze-kde-pam enable-login`. Remember it only starts when you submit the
  form.
