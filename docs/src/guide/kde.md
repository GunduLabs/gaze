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

```bash [openSUSE Tumbleweed]
sudo zypper install gaze-kde
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

`gaze-kde` puts Gaze in one of those two slots, inside a marked block:

```
# BEGIN gaze (managed by gaze-kde; remove with `gaze-kde-pam disable`)
-auth       requisite                                    pam_nologin.so
-auth       requisite                                    pam_faillock.so    preauth
-auth       [success=done default=ignore]                pam_gaze.so
# END gaze
```

The leading `-` means a missing module is skipped rather than aborting the stack,
so uninstalling Gaze cannot leave a greeter that accepts input and never answers.
The two gates are explained under [Account locking](#account-locking).

1. The greeter calls that service as soon as the lock screen appears, before you
   touch anything.
2. Gaze claims the camera through `gazed` and verifies your face.
3. On a match, `success=done` ends the stack and the screen unlocks.
4. On anything else, `default=ignore` falls through to the rest of the stack, a
   fingerprint reader if you have one, and the password field carries on waiting
   the whole time.

### Which slot Gaze takes

Normally it is `kde-fingerprint`, whose hint text is the closest thing the lock
screen has to a biometric label.

If a fingerprint reader already owns that file, `gaze-kde` uses `kde-smartcard`
instead. The two slots are the same machinery to KScreenLocker, so nothing is lost
by moving, and it buys real concurrency: `pam_fprintd` blocks for its whole 30
second timeout, so sharing one slot means whichever module runs second is starved.
Gaze would take the first twelve seconds and the reader would only then get a turn.
In separate slots both start at once and the first to match wins.

The cost is cosmetic. Plasma labels that slot "(or scan your smartcard)", which is
wrong for a camera, though Gaze replaces it with a real message the moment there is
something to say. `gaze doctor` reports which slot is in use, and `gaze-kde-pam`
never wires both.

The reader has to actually be installed for this to kick in, not merely mentioned:
every distribution's `kde-fingerprint` names `pam_fprintd` whether or not `fprintd`
is present, so the check is for the module on disk.

On openSUSE and Gentoo the smartcard slot ships `pam_pkcs11 wait_for_card`, which
blocks with no timeout. Gaze runs above it, so face unlock is unaffected, but after
a face non-match that module goes on holding the slot for the rest of the lock.

### One attempt per lock

The greeter gives a noninteractive slot a single authentication per lock: it
deliberately ignores biometric failures and only re-arms everything when a
*password* attempt fails. Because the daemon stops looking a few seconds after it
loses sight of a face, Gaze keeps retrying internally for its whole budget rather
than giving up on the first empty frame. Waking the screen and looking up a
moment later still unlocks.

### Status messages

The lock screen has no handler at all for an *informational* message from a
biometric slot, so anything you need to read ("Face not recognized", "Too dark for
face authentication") is sent as an *error* message, which briefly replaces the
slot's hint label. Gaze still sends one informational message when it starts
looking, even though nothing displays it: that is what tells the greeter this
unlock had a prompt, and without it a face match lands on an extra "Unlock" button
instead of going straight to the desktop
([bug 497904](https://bugs.kde.org/show_bug.cgi?id=497904)).

### Only one Gaze per unlock

On Fedora, Debian, and openSUSE, Gaze installs into the shared authentication
stack (`password-auth` or `common-auth`) that `/etc/pam.d/kde` also includes. Once a
biometric slot runs Gaze, the module stands down in the services that reach it
that way, so a single lock screen does not run face auth two or three times over
and have those clients fight for one camera. `kde` yields to either slot, and
`kde-smartcard` yields to `kde-fingerprint` if both somehow have a line.

Removing `gaze-kde` restores the previous behaviour automatically, because the
module decides by reading the slot files at authentication time rather than from
a build flag.

### Account locking

A `success=done` match ends the whole auth stack, so anything below Gaze in the
slot is skipped, including checks that are meant to refuse you. Gaze therefore
goes in **below** the stack's gates rather than at the top, and carries
`pam_nologin` and `pam_faillock preauth` inside its own block for the distributions
that keep those behind an `include` where inserting below them is not possible.
A locked-out or `nologin`-blocked account cannot be unlocked by face.

Where the slot can reach `pam_faillock authfail` (Fedora, through
`substack fingerprint-auth`), Gaze fails the slot outright on a non-match instead
of falling through to it, so looking away a few times cannot spend your login
attempts and lock the account. Gaze never does this in a stack that also serves
the password, where failing outright would deny a correct password whenever the
daemon is down.

::: warning An unrelated upstream bug can still lock you out
On Plasma before 6.7 (and before the 6.5.x and 6.6.x backports), a *successful*
biometric unlock made `pam_unix` in the password service report a failure, so
unlocking repeatedly in quick succession could trip `pam_faillock` and lock the
account. That is
[kscreenlocker 29d01bf7](https://invent.kde.org/plasma/kscreenlocker/-/commit/29d01bf74958b96b41d1726b5ff6b133a7a0e402),
fixing [bug 484363](https://bugs.kde.org/show_bug.cgi?id=484363), and it applies to
fingerprint readers exactly as much as to Gaze. `faillock --user "$USER"` shows
what has been recorded, and `faillock --reset` clears it.
:::

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
lock screen, and on every version shipping today it starts PAM only when you
submit the login form. So face auth there is not hands-free: press Enter with the
password field empty and look at the camera.

That is not a Gaze limitation, and it is worth being clear about it because the
comparison usually made is with fingerprint. **A fingerprint reader behaves exactly
the same way on this screen.** `pam_fprintd` goes into the same `plasmalogin` or
`sddm` stack, that stack runs on submit, and every distribution's fingerprint
instructions tell you to press Enter on an empty field before you swipe. Neither
method scans before you type, because neither one gets to decide when the greeter
calls PAM.

It is off by default. To turn it on:

```bash
sudo gaze-kde-pam enable-login
```

That inserts Gaze into `/etc/pam.d/plasmalogin` and `/etc/pam.d/sddm`, whichever
exist, after the stack's gate modules (`pam_nologin`, the `user != root` check,
`pam_selinux_permit`) so a face match cannot skip them. Turn it back off with
`sudo gaze-kde-pam disable-login`.

A service counts as existing when it is in `/usr/lib/pam.d` too, which is where
Fedora's `plasma-login-manager` ships `plasmalogin`: nothing appears under
`/etc/pam.d` there until something customises the stack, and PAM falls back to the
vendor copy meanwhile. `enable-login` reads whichever of the two the greeter would
really use, so it neither reports a missing login stack on those systems nor
writes an `/etc` file where there is no block to add.

On Fedora, Debian, and openSUSE it will report that the stack **already reaches Gaze** and
change nothing. That is correct: those login stacks include the shared
authentication stack Gaze installs into (`password-auth` or `common-auth`), so face
auth already runs at the greeter on submit. Inserting a second line would make a
failed scan run the camera twice over before the password prompt appeared. In
practice `enable-login` only has work to do on Arch, where Gaze is wired into
`sudo` and `polkit-1` alone.

### Hands-free at the greeter

Upstream is fixing this in Plasma Login Manager, not in SDDM. The work is
[plasma-login-manager!185](https://invent.kde.org/plasma/plasma-login-manager/-/merge_requests/185),
which runs password and biometric authentication as two independent helpers: the
biometric one starts as soon as the greeter has a user and a session selected, an
incorrect password does not stop it, and the first method to succeed starts the
session. It installs a dedicated `plasmalogin-fingerprint` PAM service, the exact
counterpart of `kde-fingerprint` on the lock screen.

`enable-login` already handles it. If `plasmalogin-fingerprint` exists, in
`/etc/pam.d` or in the vendor directory, Gaze goes in there, ahead of `pam_fprintd`
for the same reason as on the lock screen, and face auth then runs before you touch
anything. If it does not exist,
nothing is written and you get the submit-triggered path above. The opt-in is
remembered, so when your distribution ships a Plasma Login Manager carrying that
change, the next `gaze-kde` upgrade wires it up without you running anything.

Two things stay true even then:

- It scans for a **preselected** user only. PAM needs a username before it can
  verify anyone, so on a greeter configured to ask you to type your username there
  is nobody to scan for until you submit. This is the same constraint Gaze has at
  the GDM login screen.
- Where Gaze is in the `plasmalogin` stack as well, whether directly or through
  `password-auth`, the module stands down there once the up-front service runs it,
  so one submit does not start two scans fighting for the camera.

Nothing here applies to SDDM.
[sddm#1220](https://github.com/sddm/sddm/pull/1220) proposed the same feature in
December 2019 and is still open, and upstream's answer has been that Plasma Login
Manager is where it gets solved.

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

It edits the slot in place inside a marked block, because on most distributions
that file belongs to `plasma-workspace` and overwriting it would drop your
fingerprint reader. A `pam_gaze` line you added yourself, outside the marked
block, is left alone.

Where a distribution ships the service under `/usr/lib/pam.d` instead of `/etc`
(Arch, Debian, Ubuntu, Fedora and openSUSE all do, for one service or another),
`enable` copies it to `/etc/pam.d` first and edits the copy, because a file in
`/etc` shadows the vendor one and writing a fresh file there would silently drop
whatever the distribution had configured. `disable` deletes that copy again so the
vendor stack becomes authoritative, unless it has been changed since Gaze wrote it,
in which case only the Gaze block goes. Where no such file exists anywhere,
`enable` creates one and `disable` removes it. `enable-login` and `disable-login`
do the same for the greeter's stack, and only make the copy when they have a block
to write.

None of those vendor files are packaged as configuration files, so a later
distribution update produces no `.pacnew` or `.rpmnew` to tell you that your
`/etc` copy has gone stale. `gaze doctor` and `gaze-kde-pam status` compare the
vendor file against what was copied and warn when it has moved on.

`gaze-kde-pam disable` is remembered, so a package upgrade does not quietly turn
face unlock back on. Use `enable --force` to undo it.

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

```bash [openSUSE Tumbleweed]
sudo zypper remove gaze-kde
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
  `gaze list-faces`. Most often the daemon is not running or the current user
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
