# Uninstallation

This guide covers completely removing Gaze and all its components from your system.

## Quickest path: `gaze uninstall`

```bash
gaze uninstall
```

This runs the full cleanup sequence: reset GNOME/GDM lock and login settings, remove system and per-user copies of the GNOME extension, revert PAM, stop the daemon, remove packages (including AUR `-debug` split packages) and the repo, delete `gazed` core dumps, and wipe `/etc/gaze`, `/var/cache/gaze`, and `/var/lib/gaze`. It prints the plan and asks for confirmation first. Useful flags:

- `--keep-data`: preserve `/var/lib/gaze` (enrolled faces)
- `--dry-run`: print the plan without running anything
- `--yes`: skip the confirmation prompt

If you'd rather run the steps yourself, follow the manual procedure below.

## Step 1: Disable integrations

Before removing packages, disable any active integrations to avoid leaving your system in a broken state.

### Reset GNOME lock screen settings

```bash
gnome-extensions disable gaze@gundulabs.com 2>/dev/null || true
gnome-extensions uninstall gaze@gundulabs.com 2>/dev/null || true
gsettings reset-recursively org.gnome.shell.extensions.gaze
rm -rf ~/.local/share/gnome-shell/extensions/gaze@gundulabs.com
```

Repeat this for each desktop user who enabled lock screen face unlock. The last command removes any per-user copy of the extension (left by `gnome-extensions install` or a development checkout); without it GNOME keeps listing the extension as disabled.

### Revert hyprlock face unlock

If you enabled Gaze for hyprlock, remove the `module = hyprlock-gaze` line from the `auth { pam { ... } }` block in `~/.config/hypr/hyprlock.conf` (or restore `~/.config/hypr/hyprlock.conf.gaze-backup` if the installer created one). Repeat for every user that enabled it.

```bash
sed -i.bak '/^\s*module\s*=\s*hyprlock-gaze/d' "${XDG_CONFIG_HOME:-$HOME/.config}/hypr/hyprlock.conf"
```

### Remove GDM login defaults and overrides

```bash
sudo rm -f /etc/dconf/db/gdm.d/00-gaze-defaults* /etc/dconf/db/gdm.d/99-gaze*
sudo dconf update
```

### Revert PAM configuration

::: code-group

```bash [Debian/Ubuntu]
sudo pam-auth-update --package --remove gaze
```

```bash [Fedora]
if [ -f /etc/gaze/authselect.previous ]; then
  profile=$(sudo sed -n 's/^Profile ID:[[:space:]]*//p' /etc/gaze/authselect.previous)
  features=$(sudo sed -n 's/^- //p' /etc/gaze/authselect.previous | tr '\n' ' ')
  sudo authselect select "$profile" $features --force
else
  sudo authselect select sssd --force
fi
```

```bash [Arch Linux]
sudo sed -i '/pam_gaze/d' /etc/pam.d/sudo
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
sudo apt remove --purge gaze gaze-gui gaze-gnome-extension gaze-hyprlock
sudo apt autoremove
```

```bash [Fedora]
sudo dnf remove gaze gaze-gui gaze-gnome-extension gaze-hyprlock
```

```bash [Arch Linux / Manjaro]
sudo pacman -Rns gaze-bin gaze-gui-bin gaze-gnome-extension-bin gaze-hyprlock-bin
# AUR builds may also have installed -debug split packages:
pacman -Q | awk '/^gaze.*-debug /{print $1}' | xargs -r sudo pacman -Rns --noconfirm
```

```bash [Flatpak (GUI only)]
flatpak uninstall com.gundulabs.Gaze
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

```bash [Flatpak]
flatpak remote-delete gundulabs
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

### Systemd drop-ins

Local overrides for the daemon (debug logging, development checkouts) live in a gazed-specific directory:

```bash
sudo rm -rf /etc/systemd/system/gazed.service.d
```

### Core dumps

If `gazed` ever crashed, systemd may have saved core dumps. These can contain decrypted face templates from the daemon's memory, so remove them:

```bash
sudo find /var/lib/systemd/coredump \( -name 'core.gazed.*' -o -name 'core.gaze.*' -o -name 'core.gaze-gui.*' \) -delete
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
