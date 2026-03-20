# Installation

Use one of these paths.

## Path A: one-line installer (recommended)

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh
```

This downloads release packages and installs:

- `gaze` (daemon + CLI)
- `gaze-gui`
- `gaze-gnome-extension`

It also sets up your distro package repository and signing key so future updates work with your system package manager.

## Path B: install from Gundu Labs repositories

Use this if you want to manually configure package repositories.

::: code-group

```bash [Debian/Ubuntu]
curl -fsSL https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc \
	| gpg --dearmor \
	| sudo tee /usr/share/keyrings/gundulabs-packages.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/gundulabs-packages.gpg] https://packages.gundulabs.com/deb stable main" \
	| sudo tee /etc/apt/sources.list.d/gaze.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui gaze-gnome-extension
```

```bash [Fedora/RHEL]
sudo tee /etc/yum.repos.d/gaze.repo >/dev/null <<'EOF'
[gaze]
name=Gundu Labs Packages
baseurl=https://packages.gundulabs.com/rpm/x86_64
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc
EOF
sudo rpm --import https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc
sudo dnf install gaze gaze-gui gaze-gnome-extension
```

```bash [Arch Linux / Manjaro]
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

## Path C: GUI-only via Flatpak

```bash
flatpak remote-add --if-not-exists --no-gpg-verify gundulabs https://packages.gundulabs.com/flatpak
flatpak install gundulabs com.gundulabs.Gaze
```

Use this if you only want the GUI app. For full PAM login integration, use Path A or Path B.

This is also the recommended GUI path for Fedora Silverblue, Kinoite, and other atomic-style desktops. In those environments, it is usually better to install the GUI through Flatpak, Distrobox, or your normal app workflow instead of layering extra desktop packages unless you have a specific reason to.

## Verify installation

Run these commands after install:

```bash
systemctl status gazed
gaze --version
gaze-gui --help
```

What you should see:

- `systemctl status gazed`: the service should show as running or active
- `gaze --version`: prints the installed CLI version
- `gaze-gui --help`: confirms the GUI binary is installed correctly

If daemon is inactive:

```bash
sudo systemctl enable --now gazed
```

## First run

```bash
gaze add-face default
gaze auth --verbose
```

## PAM configuration and login manager details

See the [PAM guide](/guide/pam) for distro-specific PAM behavior, authselect setup, and manual steps.

## Enable GNOME lock screen extension

```bash
gnome-extensions enable gaze@gundulabs.com
```

On Wayland, log out and back in after extension installation or update.

## If auth still fails

Use the [troubleshooting guide](/guide/troubleshooting).

## Build from source (advanced)

Install dependencies:

::: code-group

```bash [Debian/Ubuntu]
sudo apt install libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev
```

```bash [Fedora/RHEL]
sudo dnf install opencv-devel clang-devel libv4l-devel pam-devel gtk4-devel libadwaita-devel
```

:::

Build:

```bash
cargo build --workspace --release
```

For packaging workflows:

```bash
go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest
export PATH="$PATH:$(go env GOPATH)/bin"
```
