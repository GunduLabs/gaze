<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Console login (TTY)

Gaze can authenticate the `login` prompt on a virtual terminal, the one you get
on a server with no desktop or by switching to a free VT with Ctrl+Alt+F3.

Type your username, press Enter, and look at the camera. The password prompt is
printed by `pam_unix`, not by `login` itself, so when Gaze succeeds first that
prompt never appears and you go straight to a shell.

## Setup

`login` uses the shared authentication stack, so on most distros enabling Gaze
the usual way covers it:

::: code-group

```bash [Debian/Ubuntu]
sudo pam-auth-update --package
```

```bash [Fedora and compatible]
sudo authselect select gaze with-silent-lastlog --force
```

```bash [openSUSE Tumbleweed]
sudo pam-config --add --gaze
sudo pam-config --update
```

:::

Debian's `/etc/pam.d/login` includes `common-auth`, Fedora's includes
`system-auth`, and openSUSE's includes `common-auth`, so the profile or module
you enable for `sudo` reaches the console too.

### Arch Linux

Arch needs a manual step. Gaze deliberately stays out of
`/etc/pam.d/system-auth` because `pambase` overwrites that file on upgrade (see
[PAM](/guide/pam#arch-linux-manjaro)), and `/etc/pam.d/login` reaches Gaze only
through that shared stack. Add Gaze to `/etc/pam.d/login` directly instead:

```bash
staged=$(sudo mktemp /etc/pam.d/login.gaze.XXXXXX) && \
  sudo awk -v out="$staged" '
    /^[[:space:]]*auth[[:space:]]+(include|substack)[[:space:]]/ && !inserted {
        print "auth        sufficient    pam_gaze.so" > out
        inserted = 1
    }
    { print > out }
    END { if (!inserted) exit 1 }
' /etc/pam.d/login && \
  sudo install -m 644 "$staged" /etc/pam.d/login
sudo rm -f "$staged"
```

Two details of that command matter, because a mangled `/etc/pam.d/login` locks
you out of every terminal:

- The Gaze line goes immediately above `auth include system-login`, which is the
  line that pulls in the shared stack. Everything Arch puts before it, meaning
  `pam_nologin` and `pam_securetty` where it is used, is a veto that has to run
  first. Gaze is `sufficient`, so a face match returns from the stack right there
  and nothing printed after it runs.
- `awk` writes the staged file itself rather than being piped into `tee`. A
  pipeline reports only the exit status of its last command, so a failing `awk`
  would still leave `tee` reporting success and `install` would happily replace
  your login stack with a truncated or empty file. Written this way, `&&` sees
  awk's own status, and awk exits non-zero if it never found the include line.

The staging file is created by root inside `/etc/pam.d` on purpose. Building the
new stack at a fixed path under `/tmp` would let any other local user pre-create
it, and whatever they left there would become your authentication stack.

Check the result with `cat /etc/pam.d/login` before you log out.

::: warning
Keep a root shell open while you test this. A mistake in `/etc/pam.d/login` can
lock you out of every virtual terminal.
:::

## Camera at the login prompt

No session exists before you log in, so there is no PipeWire to capture through
and no ACL granting your user the camera. Gaze notices this and captures the
seat's V4L2 device instead. `gazed` opens the camera, not the PAM module, so the
confinement that applies to `login` itself is not in the way.

Pinning `cameras.rgb` to a `pipewiresrc` pipeline will not work here. Leave it as
`primary`, which falls back to a V4L2 node when PipeWire cannot be reached, or
pin it to `usb:VVVV:PPPP` to skip the failed attempt entirely. See
[Select Camera Source](/guide/configuration#select-camera-source).

### When Gaze will not use the seat camera

Gaze takes the seat device only when logind reports that seat0 has no active
session **and** that no session on that seat belongs to another user.

Both halves matter. logind clears the active session whenever the foreground
virtual terminal holds no session, which is also true when somebody else is
logged in on a background terminal. So "no active session" on its own does not
mean the seat is free, and Gaze does not treat it that way: if another user is
logged in anywhere on the seat, console face authentication falls back to a
password.

If logind cannot be reached at all, Gaze refuses rather than guessing.

Note also that an already-open camera is not closed when a session stops being
active. A program left running in a background session can keep the device, so
capture may fail as busy even when the seat looks free.

## Time limits

`login` allows 60 seconds for the whole attempt by default (`LOGIN_TIMEOUT` in
`/etc/login.defs`) and up to three tries (`LOGIN_RETRIES`). Each face attempt can
use up to 12 seconds, and some distros add a delay after a failure, so repeated
face failures can use the budget before you have typed anything. If that happens,
`login` exits and the terminal returns to a fresh prompt.

Lower [`auth.start_delay_ms`](/guide/configuration) to nothing on this surface,
and if you routinely fall back to a password, consider leaving Gaze off the
console stack and using it for `sudo` and the lock screen instead.

## Keyring

There is no password to hand to `pam_gnome_keyring` or `pam_kwallet` when you
authenticate with your face, so a keyring unlocked at login will instead prompt
you later. On a headless or server console this usually does not matter; on a
machine where you start a desktop from the TTY, it does.

## Turning it off

On Debian and Ubuntu, drop Gaze from the shared stack and re-add it only where
you want it:

```bash
sudo pam-auth-update --disable gaze gaze-simultaneous
```

Then follow
[Selective setup](/guide/pam#selective-setup-password-at-gdm-face-authentication-for-sudo-and-polkit).
On Arch, remove the `pam_gaze.so` line you added to `/etc/pam.d/login`.
On openSUSE, remove the module from the managed stack with
`sudo pam-config --delete --gaze --gaze_grosshack && sudo pam-config --update`.
