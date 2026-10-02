<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# greetd

greetd is a minimal login manager that makes no assumptions about the session it
launches, Wayland or X11. It is a common choice on wlroots systems and on Arch,
and on NixOS, where it is one display manager among several rather than the
session manager of the distribution. It also fronts the `noctalia-greeter`
session.

greetd runs **two** PAM stacks, and which one you are looking at decides
everything else on this page:

| PAM service     | Runs as      | Purpose                                    |
| --------------- | ------------ | ------------------------------------------ |
| `greetd-greeter`| `greetd`     | Paints the login screen. `pam_permit`, never authenticates anyone. |
| `greetd`        | your user    | Authenticates the session and opens it.    |

Only `greetd` may release a keyring credential. `greetd-greeter` never does, and
`gaze doctor` will say so if you point it at the greeter's stack.

## Setup

greetd's own file has no auth entries of its own; it defers to the shared
stack, which is where Gaze goes. Enable Gaze for the shared stack as usual:

```bash
# Debian/Ubuntu/Mint
sudo pam-auth-update --package
```

```bash
# Fedora and compatible
sudo authselect select gaze with-silent-lastlog --force
```

```bash
# openSUSE Tumbleweed
sudo pam-config --add --gaze
sudo pam-config --update
```

That gives you a face login with a password fallback. The rest of this page is
about not having to type the *keyring* password afterwards.

## GNOME Keyring without a prompt

Face authentication never supplies a password, so `pam_gnome_keyring.so` finds
no `PAM_AUTHTOK` and prompts once the desktop is up. Gaze can release a
TPM-protected copy of that password instead, once you opt in and enrol:

```bash
sudo gaze config     # [storage] encrypt_templates = true
                     # [storage] unlock_gnome_keyring = true
                     # [liveness] enabled = true
sudo systemctl restart gazed
sudo gaze keyring --user "$USER"
```

Both settings are needed: the credential is sealed under the TPM, so the feature
refuses to run without template encryption, and it will not release a password
without liveness. Re-enrol after a TPM clear, a password change, or a keyring
password change.

::: warning Set `encrypt_templates` first, in a separate run
`gaze config` only asks about `unlock_gnome_keyring` once `encrypt_templates` is already
`true` and liveness is on. With template encryption off the question is simply not shown,
and `gaze keyring` then refuses with `enable GNOME Keyring unlock with gaze config first`,
which reads like a bug and is not one.

Run the wizard once to turn on `encrypt_templates`, run it again to turn on
`unlock_gnome_keyring`, then restart `gazed` and enrol. Nothing in `gaze config` checks
whether a greeter is installed, so the same ordering applies on GDM.
:::

### The PAM edit

`/etc/pam.d/greetd` ships with a keyring hook that has no token to work with:

```
auth       optional    pam_gnome_keyring.so
```

Change that one line to take the token PAM already holds:

```
auth       optional    pam_gnome_keyring.so use_authtok
```

::: warning This edit is inert for face logins until Gaze supports greetd
Half of the change is in this file and half is in `pam_gaze.so`. The module only
releases a credential for the PAM services it knows about: `gdm-face` today, and
`greetd` only from a build that carries greetd support. On 0.3.5 and earlier it
never sets `PAM_AUTHTOK` for a greetd session, so this line has nothing to
consume and a **face** login keeps prompting exactly as before.

What the edit does buy on an older build is the password path: with
`use_authtok` in place a typed password reaches `pam_gnome_keyring.so`, so the
keyring unlocks during authentication instead of prompting once the desktop is
up.
:::

Do not move it. greetd has no token-only PAM service the way GDM's `gdm-face`
does, so one stack serves both kinds of login and the keyring line has to sit
**after** whatever runs `pam_gaze.so`. On a face match the module receives Gaze's
sealed credential; after `pam_unix` it receives the password you typed. With no
token at all the module is `optional` and skips, so password login is untouched.

A `pam_gaze.so` line must not sit above it in greetd's own file if that line
ends the auth section on a match — `sufficient`, `[success=done …]` and
`[success=end …]` all do, and any of them steps straight over the keyring line,
which is exactly what `pam_deny` exists to prevent in `gdm-face`. Use a numeric
`[success=N …]` that skips only the password lines, or leave the Gaze line inside
the shared stack where the distro put it.

Verify with:

```bash
sudo gaze doctor
```

It checks the ordering above, not just that the line is present.

::: warning Where the released token is visible
`gdm-face` is a token-only PAM service: its whole stack exists to carry the released
password to `pam_gnome_keyring.so`. greetd's stack is the user session stack, so on a
face login `PAM_AUTHTOK` holds the released credential for the rest of greetd's auth
section.

That remainder is small, and narrower than it first looks. `pam_gaze.so` is `auth
sufficient` in `system-auth`, so a match ends the substack there and `pam_unix` never
runs — the password module is never offered the stored credential. What follows it is:

- `auth optional pam_gnome_keyring.so use_authtok` — the intended consumer.
- the `-`-prefixed `pam_kwallet5.so` and `pam_kwallet.so` lines, which Fedora ships
  commented out and a KDE install would enable. These do read the token.
- `auth include postlogin` — the same include `gdm-face` has, whose session modules
  (`pam_umask`, `pam_lastlog2`) do not read it.

So the one module that could newly see the credential is a KWallet module, and only on a
KDE install. If you would rather it did not, leave `unlock_gnome_keyring` off and accept
the prompt.
:::


### SELinux

On Fedora the credential lives under `/var/lib/gaze/keyring`, and the greeter has
to read it. Gaze ships the policy:

```bash
sudo semodule -i /usr/share/gaze/gaze-greeter-keyring.pp
```

`gaze doctor` reports this as **Keyring SELinux policy** when it is enforcing and
the module is missing. A missing module does not fail the login — it silently
falls back to the keyring password.

## Troubleshooting

`gkr-pam: no password is available for user` in `journalctl -b` means the
`auth` line has no `use_authtok`, or `pam_gaze.so` is not reached from
`/etc/pam.d/greetd` at all. Both are what the doctor **Keyring** check looks
for.

`Face auth requested service="greetd"` in the same log confirms the greeter
authenticated through the session stack, which is the one that should unlock the
keyring.
