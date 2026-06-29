<div align="center">

<img src="packaging/gui/com.gundulabs.Gaze.svg" alt="Gaze icon" width="120" />

# Gaze

**Facial authentication for Linux**

[![CI](https://github.com/gundulabs/gaze/actions/workflows/ci.yml/badge.svg)](https://github.com/gundulabs/gaze/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[Documentation](https://gaze.gundulabs.com) · [Install](https://gaze.gundulabs.com/guide/installation) · [Development](https://gaze.gundulabs.com/guide/development)

</div>

---

> [!NOTE]
> Gaze includes local liveness anti-spoofing and support for infrared (IR) cameras to secure authentication against spoofing attacks. For high-security environments, it is recommended to keep standard system authentication active as a fallback.

Facial authentication for Linux with on-device face recognition, PAM integration, and tools for login, lock screen, sudo, and desktop management.

## Install

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

The installer installs the Gaze daemon, CLI, and GUI. It installs the GNOME Shell extension only when it detects a GNOME desktop session; on KDE Plasma and other non-GNOME desktops it skips GNOME-specific packages so it does not pull in GNOME Shell. If you installed the GNOME extension manually or automatic enablement was not possible, reboot (so GNOME Shell scans the new extension) and then run from GNOME:

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

> Running `gnome-extensions enable` before rebooting will return `Extension "gaze@gundulabs.com" does not exist`. Shell only rescans extension directories at session start.

<details>
<summary>Manual install (Debian/Ubuntu, Fedora, Arch/Manjaro/CachyOS)</summary>

**Debian / Ubuntu**

```bash
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.gpg \
  | sudo tee /usr/share/keyrings/gundulabs-archive-keyring.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] https://packages.gundulabs.com/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui
```

**Fedora**

```bash
sudo rpm --import https://packages.gundulabs.com/keys/gundulabs-repo.asc
sudo tee /etc/yum.repos.d/gundulabs.repo >/dev/null <<'EOF'
[gundulabs]
name=Gundu Labs
baseurl=https://packages.gundulabs.com/rpm/fedora/$releasever/$basearch
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://packages.gundulabs.com/keys/gundulabs-repo.asc
EOF
sudo dnf makecache
sudo dnf install gaze gaze-gui
```

**Arch / Manjaro / CachyOS**

```bash
# Requires an AUR helper such as yay or paru. yay shown here.
yay -S --needed gaze-bin gaze-gui-bin
```

**Flatpak (GUI only; also install one of the system packages above for the `gazed` daemon)**

```bash
flatpak install --from https://packages.gundulabs.com/flatpak/com.gundulabs.Gaze.flatpakref
```

For GNOME lock screen face unlock after manual package installation, also install `gaze-gnome-extension` (`gaze-gnome-extension-bin` on Arch), reboot, then from your GNOME session run `gnome-extensions enable gaze@gundulabs.com` and `gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true`. On KDE Plasma, use the base packages and follow the PAM guide for login/lock integration.

</details>

After installation (any method), reboot once to ensure all system-level changes are fully applied.

```bash
sudo reboot
```

## Quick start

```bash
# Enroll your face
gaze add-face default

# Test authentication
gaze auth

# Or use the GUI
gaze-gui
```

## How it works

Gaze runs a daemon (`gazed`) that communicates over DBus. When authentication is requested (by PAM at login, the GNOME extension on the lock screen, or the CLI), the daemon captures a frame from your webcam, detects and aligns the face, computes an embedding using an ONNX model, and compares it against stored enrollments.

All processing happens locally. Face embeddings are stored on disk, not transmitted anywhere.

```
Camera → Face Detection (SCRFD) → Alignment → Embedding (ArcFace) → Match → Liveness (MiniFASNet-V2)
```

## Components

| Component | Description |
|-----------|-------------|
| `gazed` | System daemon exposing `com.gundulabs.Gaze` on DBus |
| `gaze` | CLI for enrollment and authentication (crate: `gaze-cli`) |
| `gaze-gui` | GTK4/Adwaita graphical application |
| `pam-gaze` | PAM module for login/lock screen integration |
| `gaze-gnome-extension` | GNOME Shell extension for lock screen auth |
| `gaze-hyprlock` | PAM service for hyprlock face unlock on Hyprland |

## Configuration

```toml
# /etc/gaze/config.toml
[security]
level = "medium"    # low | medium | high | maximum | custom

[cameras]
rgb = "primary"
dark_luma_threshold = 30

[auth]
abort_if_ssh = true
abort_if_lid_closed = true

[enrollment]
max_templates = 2

[liveness]
enabled = true
threshold = 0.8
```

See the [configuration guide](https://gaze.gundulabs.com/guide/configuration) for all options.

## CLI usage

```
gaze add-face <name>         Enroll a new face
gaze refine-face <name>      Add samples to an existing enrollment
gaze auth                    Authenticate
gaze auth --verbose          Authenticate with detailed metrics
gaze list-faces              List enrolled faces
gaze rename-face <old> <new> Rename a face
gaze remove-face <name>      Remove a face
gaze clear-user              Remove all face data for current user
gaze config                  Interactive configuration editor
gaze config --show           Print current config and exit
gaze uninstall               Completely remove Gaze (packages, PAM, config, models, data)
gaze uninstall -y            Skip confirmation prompt
```

## Building from source

**Dependencies:** Rust 1.85+, [`just` 1.51+](https://github.com/casey/just), [`nfpm`](https://nfpm.goreleaser.com)

```bash
# Ubuntu/Debian
sudo apt install build-essential pkg-config clang libclang-dev \
  libopencv-dev libv4l-dev libpam0g-dev \
  libgtk-4-dev libadwaita-1-dev \
  libcairo2-dev libglib2.0-dev libgdk-pixbuf-2.0-dev libpango1.0-dev libgraphene-1.0-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev

# Build
just build-rust

# Package
just package <deb | rpm | archlinux>
```

See the [development guide](https://gaze.gundulabs.com/guide/development) for more.

## License

[MIT](LICENSE)
