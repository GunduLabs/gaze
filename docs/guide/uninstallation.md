# Uninstallation

This guide covers completely removing Gaze and all its components from your system.

## Step 1: Disable integrations

Before removing packages, disable any active integrations to avoid leaving your system in a broken state.

### Disable the GNOME extension

```bash
gnome-extensions disable gaze@gundulabs.com
```

### Disable face auth at GDM login (if enabled)

```bash
sudo -u gdm dbus-run-session gsettings set org.gnome.login-screen.gaze enable-face-authentication false
```

### Revert PAM configuration

::: code-group

```bash [Debian/Ubuntu]
sudo pam-auth-update --package --remove gaze
```

```bash [Fedora/RHEL]
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

```bash [Fedora/RHEL]
sudo dnf remove gaze gaze-gui gaze-gnome-extension
```

```bash [Arch Linux / Manjaro]
sudo pacman -Rns gaze gaze-gui gaze-gnome-extension
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

```bash [Fedora/RHEL]
sudo rm /etc/yum.repos.d/gundulabs.repo
sudo rpm -e gpg-pubkey-$(rpm -qa gpg-pubkey --qf '%{NAME}-%{VERSION}-%{RELEASE}\t%{SUMMARY}\n' | grep -i gundulabs | awk '{print $1}' | sed 's/gpg-pubkey-//')
sudo dnf makecache
```

```bash [Arch Linux / Manjaro]
# Remove the repository from pacman.conf
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

### Face enrollment data

```bash
sudo rm -rf /var/lib/gaze
```

### Downloaded ML models

```bash
sudo rm -rf /opt/gaze
```

### Cache

```bash
sudo rm -rf /var/cache/gaze
```

### Configuration

```bash
sudo rm -rf /etc/gaze
```

### SELinux policy (Fedora/RHEL only)

```bash
sudo semodule -r gaze-gdm-camera
```

### Recompile GSettings schemas

After the GNOME extension package is removed, recompile schemas to clean up:

```bash
sudo glib-compile-schemas /usr/share/glib-2.0/schemas
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
