# Installation

## Install from Gundu Labs repositories

This guide assumes a shared package endpoint at `https://packages.gundulabs.com` (backed by Cloudflare R2).

::: code-group

```bash [Debian/Ubuntu]
curl -fsSL https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc \
	| gpg --dearmor \
	| sudo tee /usr/share/keyrings/gundulabs-packages.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/gundulabs-packages.gpg] https://packages.gundulabs.com/deb stable main" | sudo tee /etc/apt/sources.list.d/gaze.list
sudo apt update
sudo apt install gaze gaze-gui gaze-gnome-extension
```

```bash [Fedora/RHEL]
sudo tee /etc/yum.repos.d/gaze.repo >/dev/null <<'EOF'
[gaze]
name=Gaze Packages
baseurl=https://packages.gundulabs.com/rpm/x86_64
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc
EOF
sudo rpm --import https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc
sudo dnf install gaze gaze-gui gaze-gnome-extension
```

```bash [Arch Linux]
sudo tee /etc/pacman.d/gaze-mirrorlist >/dev/null <<'EOF'
Server = https://packages.gundulabs.com/arch/x86_64
EOF
curl -fsSL https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc -o /tmp/gundulabs-packages.asc
sudo pacman-key --add /tmp/gundulabs-packages.asc
sudo pacman-key --lsign-key "$(gpg --show-keys --with-colons /tmp/gundulabs-packages.asc | awk -F: '/^fpr:/ {print $10; exit}')"
rm -f /tmp/gundulabs-packages.asc
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[gaze]
SigLevel = Required DatabaseOptional
Include = /etc/pacman.d/gaze-mirrorlist
EOF
sudo pacman -Sy gaze gaze-gui gaze-gnome-extension
```

:::

After installation from packages, enable and start the daemon:

```bash
sudo systemctl enable --now gazed
```

## Manual install from local build artifacts

## Flatpak GUI install

```bash
flatpak remote-add --if-not-exists --no-gpg-verify gundulabs https://packages.gundulabs.com/flatpak
flatpak install gundulabs com.gundulabs.Gaze
```

## 1. Install binaries and enable the daemon

```bash
sudo cp target/release/gazed /usr/bin/gazed
sudo cp target/release/gaze /usr/bin/gaze
sudo cp target/release/gaze-gui /usr/bin/gaze-gui
sudo cp dist/gazed.service /etc/systemd/system/
sudo systemctl enable --now gazed
```

## 2. Install the DBus policy

```bash
sudo cp dist/org.gaze.Auth.conf /etc/dbus-1/system.d/
```

## 3. Install the config

```bash
sudo mkdir -p /etc/gaze
sudo cp dist/config.toml /etc/gaze/config.toml
```

## 4. Install the PAM modules

::: code-group

```bash [Fedora/RHEL (x86_64)]
sudo cp target/release/libpam_gaze.so /usr/lib64/security/pam_gaze.so
sudo cp target/release/libpam_gaze_grosshack.so /usr/lib64/security/pam_gaze_grosshack.so
```

```bash [Debian/Ubuntu]
sudo cp target/release/libpam_gaze.so /lib/x86_64-linux-gnu/security/pam_gaze.so
sudo cp target/release/libpam_gaze_grosshack.so /lib/x86_64-linux-gnu/security/pam_gaze_grosshack.so
```

```bash [Arch Linux]
sudo cp target/release/libpam_gaze.so /usr/lib/security/pam_gaze.so
sudo cp target/release/libpam_gaze_grosshack.so /usr/lib/security/pam_gaze_grosshack.so
```

:::

## 5. Enable face authentication

::: code-group

```bash [Fedora/RHEL]
sudo authselect select vendor/gaze --force
```

```bash [Debian/Ubuntu]
sudo cp dist/pam-configs/gaze dist/pam-configs/gaze-simultaneous /usr/share/pam-configs/
sudo pam-auth-update --package
```

:::

This configures `system-auth` and `password-auth` to include `pam_gaze.so`, covering both login and lock screen unlock via GDM.

## 6. Enable the GNOME Shell extension

```bash
gnome-extensions enable gaze@gundulabs.com
```

The extension hooks into GDM to trigger face auth on the lock screen using `/etc/pam.d/gdm-face`. It also installs a SELinux policy that allows GDM to access the camera.

::: warning Wayland note
On Wayland, GNOME Shell must be restarted (log out and back in) before it picks up a newly installed system extension.
:::

## One-shot rebuild & reinstall (development)

Requires [`cargo-nfpm`](https://crates.io/crates/cargo-nfpm):

```bash
cargo install cargo-nfpm --locked
```

Then:

```bash
./dev-reinstall.sh
```

The script auto-detects your distro (Fedora/RHEL, Debian/Ubuntu, Arch) and runs the appropriate packager and installer.
