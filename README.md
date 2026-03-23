<div align="center">

<img src="packaging/gui/com.gundulabs.Gaze.svg" alt="Gaze icon" width="120" />

# Gaze

**Facial authentication for Linux.**

[![CI](https://github.com/gundulabs/gaze/actions/workflows/ci.yml/badge.svg)](https://github.com/gundulabs/gaze/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

[Documentation](https://gaze.gundulabs.com) · [Install](https://gaze.gundulabs.com/guide/install) · [Contributing](CONTRIBUTING.md)

</div>

---

> [!WARNING]
> Gaze can currently be spoofed with a photo. Do not use it as your only authentication factor. Liveness detection and IR camera support are planned.

Gaze is a face authentication system for Linux. It runs entirely on-device with no cloud dependency, integrates with PAM for login and lock screen, and works with any standard webcam.

## Install

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

<details>
<summary>Manual install (Debian/Ubuntu, Fedora/RHEL, Arch)</summary>

**Debian / Ubuntu**

```bash
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.gpg \
  | sudo tee /usr/share/keyrings/gundulabs-archive-keyring.gpg >/dev/null
curl -fsSL https://packages.gundulabs.com/setup/deb/gundulabs.list \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui gaze-gnome-extension
```

**Fedora / RHEL**

```bash
sudo rpm --import https://packages.gundulabs.com/keys/gundulabs-repo.asc
sudo curl -fsSL https://packages.gundulabs.com/setup/rpm/gundulabs.repo \
  -o /etc/yum.repos.d/gundulabs.repo
sudo dnf makecache
sudo dnf install gaze gaze-gui gaze-gnome-extension
```

**Arch / Manjaro**

```bash
sudo tee /etc/pacman.d/gaze-mirrorlist >/dev/null <<'EOF'
Server = https://packages.gundulabs.com/arch/x86_64
EOF
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.asc -o /tmp/gundulabs-packages.asc
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

**Flatpak (GUI only)**

```bash
flatpak remote-add --if-not-exists --no-gpg-verify gundulabs https://packages.gundulabs.com/flatpak
flatpak install gundulabs com.gundulabs.Gaze
```

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

Gaze runs a daemon (`gazed`) that communicates over DBus. When authentication is requested — by PAM at login, the GNOME extension on the lock screen, or the CLI — the daemon captures a frame from your webcam, detects and aligns the face, computes an embedding using an ONNX model, and compares it against stored enrollments.

All processing happens locally. Face embeddings are stored on disk, not transmitted anywhere.

```
Camera → Face Detection (SCRFD) → Alignment → Embedding (ArcFace) → Match
```

## Components

| Component | Description |
|-----------|-------------|
| `gazed` | System daemon exposing `org.gaze.Auth` on DBus |
| `gaze` | CLI for enrollment and authentication |
| `gaze-gui` | GTK4/Adwaita graphical application |
| `pam-gaze` | PAM module for login/lock screen integration |
| `gaze-gnome-extension` | GNOME Shell extension for lock screen auth |

## Configuration

```toml
# /etc/gaze/config.toml
level = "medium"    # low | medium | high | maximum | custom

[cameras]
rgb = "/dev/video0"

[enrollment]
max_captures_per_face = 8
```

See the [configuration guide](https://gaze.gundulabs.com/guide/configuration) for all options.

## CLI usage

```
gaze add-face <name>         Enroll a new face
gaze refine-face <name>      Add samples to an existing enrollment
gaze auth                    Authenticate
gaze auth --verbose          Authenticate with similarity scores
gaze list-faces              List enrolled faces
gaze remove-face <name>      Remove a face
gaze clear-user              Remove all face data for current user
```

## Building from source

**Dependencies:** Rust 1.70+, [`just`](https://github.com/casey/just), [`nfpm`](https://nfpm.goreleaser.com)

```bash
# Ubuntu/Debian
sudo apt install build-essential libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev

# Build
cargo build --workspace --release

# Package
just package <deb | rpm | archlinux>
```

See the [development guide](https://gaze.gundulabs.com/guide/development) for more.

## License

[MIT](LICENSE)
