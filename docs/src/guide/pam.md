<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

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
the initial GDM login so that GNOME Keyring is unlocked normally (see
[Login warning](/guide/gnome#login-warning-gnome-keyring)), while still using
Gaze for privilege elevation and graphical Polkit prompts.

Enabling the Debian/Ubuntu Gaze profile through `pam-auth-update` adds Gaze to
the shared `common-auth` stack. Because `gdm-password` also includes
`common-auth`, this may make Gaze run during the initial desktop login.

The following setup was manually verified on Ubuntu 26.04 with GNOME 50.

::: warning
PAM configuration errors can prevent authentication. Keep an active root
shell open, keep password authentication enabled, and create backups before
editing these files.
:::

First disable the shared Gaze profiles:

```bash
sudo pam-auth-update --disable gaze gaze-simultaneous
```

`--disable` (rather than `--remove`) records the choice, so the
`pam-auth-update --package` call in the Gaze package's post-install script will
not re-enable the profile on the next upgrade.

Verify that Gaze is no longer present in the shared stack:

```bash
grep -n pam_gaze /etc/pam.d/common-auth \
  || echo "Gaze is not enabled in common-auth"
```

Keep the GDM face-login switch disabled. The switch lives in the Gaze extension
preferences (see
[Disable face at GDM login](/guide/gnome#disable-face-at-gdm-login)); the daemon
writes the override below when it is on, so remove it if it is already present:

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

Both files are dpkg conffiles, so a `sudo` package upgrade may prompt about the
local modification. Keep the modified version to retain face authentication.

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

A file in `/etc/pam.d` shadows the vendor file permanently, so this override
will not pick up upstream changes to the Polkit stack. Diff it against
`/usr/lib/pam.d/polkit-1` after Polkit upgrades.

Finally, confirm Gaze still sees a live PAM wiring. `gaze doctor` scans every
file in `/etc/pam.d`, so a per-service setup satisfies its PAM check:

```bash
gaze doctor
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

::: tip KDE lock screen needs `gaze-kde` to be hands-free
Being in `password-auth` means Gaze runs when KDE's lock screen authenticates, but on its own that only happens once you submit the password field. For face unlock that starts by itself, install `gaze-kde`, which runs Gaze in the slot KScreenLocker starts up front for biometrics. It also stops Gaze running twice on one lock screen, since `/etc/pam.d/kde` includes `password-auth`. See the [KDE Plasma guide](/guide/kde).
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

## openSUSE Tumbleweed

The openSUSE package ships a `pam-config` definition for Gaze and enables it
in its post-install script. This adds Gaze to the managed `common-auth` stack,
covering `sudo`, GDM, the GNOME lock screen, and other PAM services that include
`common-auth`. To apply it again after changing PAM modules, run:

```bash
sudo pam-config --add --gaze
sudo pam-config --update
```

The `--gaze` option is provided by the Gaze package's definition under
`/usr/lib/pam-config.d`. If `pam-config` reports an unknown option, confirm
that the base `gaze` package (not only `gaze-gui` or the GNOME extension) is
installed.

For simultaneous face and password authentication, enable
`pam_gaze_grosshack.so` instead (do not enable both modules):

```bash
sudo pam-config --delete --gaze
sudo pam-config --add --gaze_grosshack
sudo pam-config --update
```

Check that the managed file contains Gaze and that the common-auth link still
points at the generated file:

```bash
grep pam_gaze /etc/pam.d/common-auth-pc
readlink -f /etc/pam.d/common-auth
gaze doctor
```

Keep a root shell open while testing changes to the shared authentication
stack. If `common-auth` is not managed by `pam-config` on your installation,
follow [Other distros (manual)](#other-distros-manual) and add
`pam_gaze.so` to the service-specific PAM files instead.

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

Debian/Ubuntu and Fedora ship their own `polkit-1` PAM service and do not use `system-auth` the way Arch does, so Gaze never writes this file there. On those systems polkit picks up face auth through the shared auth stack (`pam-auth-update` on Debian/Ubuntu, the `gaze` authselect feature on Fedora). Recent Debian and Ubuntu releases ship that file as a vendor default in `/usr/lib/pam.d/polkit-1` instead of `/etc/pam.d/polkit-1`, but it still includes `common-auth`, so the shared-stack route works either way. An explicit `/etc/pam.d/polkit-1` override is only needed there if you deliberately took Gaze out of the shared stack, as in [Selective setup](#selective-setup-password-at-gdm-face-authentication-for-sudo-and-polkit).

## Other distros (manual)

Edit your shared auth stack (for example `/etc/pam.d/system-auth` on Fedora or
Arch, or `/etc/pam.d/common-auth-pc` on openSUSE when `pam-config` is not
available) and place Gaze before `pam_unix.so`.

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

## Browser extensions through Polkit (Bitwarden)

A browser extension cannot call PAM directly. Bitwarden's browser extension
hands an unlock request to the running Bitwarden desktop app through native
messaging. On Linux, the desktop app asks Polkit to authorize the
`com.bitwarden.Bitwarden.unlock` action, and the graphical Polkit agent runs the
normal `polkit-1` PAM service. Gaze therefore needs no Firefox-, Chromium-, or
Zen-specific hook: if Gaze is in the `polkit-1` stack, the request follows the
same path as any other graphical authentication prompt.

Set up and test the layers in order:

1. Follow the Polkit setup for your distribution above, then check that a plain
   Polkit request starts Gaze:

   ```bash
   gaze doctor
   pkexec /usr/bin/true
   ```

2. In the Bitwarden desktop app, enable **Unlock with system authentication**
   and **Allow browser integration**. Keep the desktop app running, logged in,
   and unlocked while setting up the extension.
3. In the browser extension, open **Settings → Account security**, enable
   **Unlock with biometrics**, and approve the connection in the desktop app.
   See [Bitwarden's biometric unlock guide](https://bitwarden.com/help/biometrics/)
   for browser and package-specific requirements.

If desktop unlock already uses Gaze but the browser extension never opens a
Polkit dialog, the request has not reached PAM. Check the native-messaging setup,
whether the desktop app is running and logged in, and whether Bitwarden supports
that browser and installation method. Adding another Gaze PAM entry cannot fix a
native-messaging failure.

If a Polkit prompt appears but Gaze does not run, return to the distribution's
Polkit setup above. `pkexec /usr/bin/true` must use Gaze first. To confirm that a
Bitwarden attempt reached the daemon through the expected service, check:

```bash
sudo journalctl -u gazed -b --no-pager | grep 'service="polkit-1"'
```

Do not add a Polkit rule that automatically authorizes Bitwarden or a browser.
That would bypass authentication rather than route it to Gaze. Bitwarden owns
the desktop/native-messaging trust boundary; Gaze only supplies face
authentication when Polkit invokes PAM.

## Safety notes

- Keep password auth enabled while testing.
- Keep a root shell open before changing PAM.
- Back up PAM files first so you can restore quickly.
