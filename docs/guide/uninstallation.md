# Uninstallation

This guide covers completely removing Gaze and all its components from your system.

## Quickest path: `gaze uninstall`

```bash
gaze uninstall
```

This runs the full cleanup sequence (reset GNOME/GDM lock and login settings, revert PAM, stop daemon, remove packages and repo, wipe `/etc/gaze`, `/var/cache/gaze`, and `/var/lib/gaze`). It prints the plan and asks for confirmation first. Useful flags:

- `--keep-data` — preserve `/var/lib/gaze` (enrolled faces)
- `--dry-run` — print the plan without running anything
- `--yes` — skip the confirmation prompt

If you'd rather run the steps yourself, follow the manual procedure below.

## Step 1: Disable integrations

Before removing packages, disable any active integrations to avoid leaving your system in a broken state.

### Reset GNOME lock screen settings

```bash
gnome-extensions disable gaze@gundulabs.com 2>/dev/null || true
gsettings reset-recursively org.gnome.shell.extensions.gaze
```

Repeat this for each desktop user who enabled lock screen face unlock.

### Remove GDM login defaults and overrides

```bash
sudo rm -f /etc/dconf/db/gdm.d/00-gaze-defaults /etc/dconf/db/gdm.d/99-gaze
sudo dconf update
```

### Revert PAM configuration

::: code-group

```bash [Debian/Ubuntu]
sudo pam-auth-update --package --remove gaze
```

```bash [Fedora]
sudo authselect select sssd --force
```

```bash [Manual PAM setup]
# Remove any pam_gaze.so or pam_gaze_grosshack.so lines
# from /etc/pam.d/system-auth or wherever you added them.
sudo nano /etc/pam.d/system-auth
```

:::

### Stop and disable the daemon

```bash
sudo systemctl stop gazed
sudo systemctl disable gazed
```

## Step 2: Remove packages

::: code-group

```bash [Debian/Ubuntu]
sudo apt remove --purge gaze gaze-gui gaze-gnome-extension
sudo apt autoremove
```

```bash [Fedora]
sudo dnf remove gaze gaze-gui gaze-gnome-extension
```

```bash [Arch Linux / Manjaro]
sudo pacman -Rns gaze-bin gaze-gui-bin gaze-gnome-extension-bin
```

:::

## Step 3: Remove the package repository

::: code-group

```bash [Debian/Ubuntu]
sudo rm /etc/apt/sources.list.d/gundulabs.list
sudo rm /usr/share/keyrings/gundulabs-archive-keyring.gpg
sudo apt update
```

```bash [Fedora]
sudo rm /etc/yum.repos.d/gundulabs.repo
sudo rpm -e gpg-pubkey-$(rpm -qa gpg-pubkey --qf '%{NAME}-%{VERSION}-%{RELEASE}\t%{SUMMARY}\n' | grep -i gundulabs | awk '{print $1}' | sed 's/gpg-pubkey-//')
sudo dnf makecache
```

```bash [Arch Linux / Manjaro]
# AUR installs do not add a Gundu Labs pacman repo.
# Only run this if you previously configured the old pacman repo.
sudo sed -i '/^\[gaze\]/,/^$/d' /etc/pacman.conf
sudo rm -f /etc/pacman.d/gaze-mirrorlist
sudo pacman -Sy
```

:::

## Step 4: Remove leftover data

Package removal does not delete user data, downloaded models, or configuration files that were modified. Remove these manually if you want a clean slate.

Refresh compiled GNOME settings after package removal if your package manager did not run the hook:

```bash
sudo dconf update
sudo glib-compile-schemas /usr/share/glib-2.0/schemas
```

### Face enrollment data

```bash
sudo rm -rf /var/lib/gaze
```

### Downloaded ML models and cache

```bash
sudo rm -rf /var/cache/gaze
```

### Configuration

```bash
sudo rm -rf /etc/gaze
```

### SELinux policy (Fedora/RPM systems only)

```bash
sudo semodule -r gaze-gdm-camera
```

## Step 5: Reload system services

```bash
sudo systemctl daemon-reload
```

## Verify removal

```bash
# All of these should fail with "command not found"
gaze --version
gazed --version
gaze-gui --help

# Should show "inactive" or "not found"
systemctl status gazed
```
