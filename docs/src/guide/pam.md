# PAM

This page is about normal PAM integration (`sudo`, polkit, shared auth stacks).

`gaze auth` is useful, but it is only a daemon/camera test. It does not run through PAM.

If you specifically want GNOME lock screen or GDM login behavior, use the [GNOME Extension guide](/guide/gnome).

## What Gaze installs

- `pam_gaze.so` (sequential mode, recommended)
- `pam_gaze_grosshack.so` (simultaneous mode)

Sequential means face auth runs first, then password fallback.
Simultaneous means face auth and password prompt run in parallel.

## Debian / Ubuntu

Packages install PAM profiles for `pam-auth-update`.

Apply or re-apply them:

```bash
sudo pam-auth-update --package
```

Pick one of the Gaze entries, then test with a real PAM prompt:

```bash
sudo -v
```

If camera opens and face auth runs, PAM wiring is active.

### Selective setup: password at GDM, face authentication for sudo and Polkit

On GNOME systems, some users may want to keep password authentication at
the initial GDM login so that GNOME Keyring is unlocked normally, while still
using Gaze for privilege elevation and graphical Polkit prompts.

Enabling the Debian/Ubuntu Gaze profile through `pam-auth-update` adds Gaze to
the shared `common-auth` stack. Because `gdm-password` also includes
`common-auth`, this may make Gaze run during the initial desktop login.

The following setup was manually verified on Ubuntu 26.04 with GNOME 50.

> [!WARNING]
> PAM configuration errors can prevent authentication. Keep an active root
> shell open, keep password authentication enabled, and create backups before
> editing these files.

First disable the shared Gaze profiles:

```bash
sudo pam-auth-update --disable gaze gaze-simultaneous
```

Verify that Gaze is no longer present in the shared stack:

```bash
grep -n pam_gaze /etc/pam.d/common-auth \
  || echo "Gaze is not enabled in common-auth"
```

Keep the GDM face-login switch disabled:

```bash
sudo rm -f /etc/dconf/db/gdm.d/99-gaze*
sudo dconf update
```

#### sudo

Back up `/etc/pam.d/sudo`, then add this line immediately before
`@include common-auth`:

```text
auth    sufficient    pam_gaze.so
```

The relevant part should look like:

```text
auth    sufficient    pam_gaze.so
@include common-auth
```

Test it with:

```bash
sudo -k
sudo -v
```

The same change can be applied to `/etc/pam.d/sudo-i` if face authentication
is also wanted for `sudo -i`.

#### Polkit

If `/etc/pam.d/polkit-1` does not exist but the vendor file is available,
create a local override:

```bash
sudo install -o root -g root -m 0644 \
  /usr/lib/pam.d/polkit-1 \
  /etc/pam.d/polkit-1
```

Add the following line immediately before `@include common-auth`:

```text
auth    sufficient    pam_gaze.so
```

Restart Polkit and test a graphical authentication request:

```bash
sudo systemctl restart polkit
pkexec /usr/bin/true
```

With this arrangement:

- GDM login uses the normal account password.
- GNOME Keyring is unlocked during login.
- `sudo` and `sudo -i` can use face authentication.
- GNOME Settings, package-management applications, and other Polkit clients
  can use face authentication.
- The GNOME extension can remain enabled for face unlock on the lock screen.
- Password authentication remains available as a fallback.

## Fedora and compatible RPM systems

RPM packages install an authselect profile at:

`/usr/share/authselect/vendor/gaze`

The profile adds Gaze to both shared authentication stacks: `system-auth`, used by tools such as `sudo`, and `password-auth`, used by KDE's lock screen, SDDM, and Plasma Login Manager. RPM upgrades refresh these generated PAM files automatically when the Gaze profile is active.

::: warning KDE lock screen is not hands-free
Being in `password-auth` means Gaze runs when KDE's lock screen authenticates, but it does not make face unlock automatic. KDE's screen locker only starts PAM authentication after you submit the password field, so the camera does not activate until you enter (or submit an empty) password. Hands-free face unlock on the KDE lock screen is not currently supported — unlike GNOME, which drives it through the Gaze Shell extension. Face auth still works for `sudo`, polkit, and other PAM prompts.
:::

Enable it:

```bash
sudo authselect select gaze with-silent-lastlog --force
```

Or simultaneous mode:

```bash
sudo authselect select gaze with-face-simultaneous with-silent-lastlog --force
```

Verify profile + PAM behavior:

```bash
sudo authselect current
sudo -v
```

## Arch Linux / Manjaro

The one-liner installer and the AUR package post-install script both configure `/etc/pam.d/sudo` automatically, inserting `pam_gaze.so` before the existing `auth include system-auth` line.

If you need to apply or re-apply it manually:

```bash
sudo awk '
    /^[[:space:]]*auth[[:space:]]/ && !done {
        print "auth        sufficient    pam_gaze.so"
        done = 1
    }
    { print }
' /etc/pam.d/sudo | sudo tee /tmp/pam-sudo-new && sudo install -m 644 /tmp/pam-sudo-new /etc/pam.d/sudo
```

Then test:

```bash
sudo -v
```

::: warning pambase updates
`/etc/pam.d/system-auth` is owned by the `pambase` package and gets overwritten on system upgrades. Gaze is added to `/etc/pam.d/sudo` directly to avoid this, but if you manually added `pam_gaze.so` to `system-auth` it will be lost on `pambase` updates.
:::

### Polkit (graphical "Authentication Required" prompts)

Arch's `polkit` package ships no `/etc/pam.d/polkit-1`, so the `polkit-1` PAM service falls back to the vendor default at `/usr/lib/pam.d/polkit-1`, which just does `include system-auth`. Since Gaze avoids patching `system-auth` (see above), graphical polkit prompts (`pkexec`, GNOME Settings, package manager GUIs, etc.) don't get face auth unless a `/etc/pam.d/polkit-1` override is installed too. The Arch package and `dev-link-system.sh` create one automatically, and only on Arch:

```text
#%PAM-1.0
auth       sufficient   pam_gaze.so
auth       include      system-auth
account    include      system-auth
password   include      system-auth
session    include      system-auth
```

Verify with:

```bash
sudo systemctl restart polkit
pkexec true
```

Debian/Ubuntu, Fedora and other distributions ship their own `/etc/pam.d/polkit-1` and do not use `system-auth` the way Arch does, so Gaze never writes this file there. On those systems polkit picks up face auth through the shared auth stack (`pam-auth-update` on Debian/Ubuntu, the `gaze` authselect feature on Fedora).

## Other distros (manual)

Edit your shared auth stack (for example `/etc/pam.d/system-auth`) and place Gaze before `pam_unix.so`.

Sequential:

```text
auth    sufficient    pam_gaze.so
auth    sufficient    pam_unix.so try_first_pass nullok
```

Simultaneous:

```text
auth    sufficient    pam_gaze_grosshack.so
auth    sufficient    pam_unix.so try_first_pass nullok
```

Then test with `sudo -v`.

## Safety notes

- Keep password auth enabled while testing.
- Keep a root shell open before changing PAM.
- Back up PAM files first so you can restore quickly.
