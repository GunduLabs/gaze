<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

<div align="center">

<img src="packaging/gui/com.gundulabs.Gaze.svg" alt="Gaze icon" width="120" />

# Gaze

**Facial authentication for Linux**

[![CI](https://github.com/gundulabs/gaze/actions/workflows/ci.yml/badge.svg)](https://github.com/gundulabs/gaze/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

[Documentation](https://gaze.gundulabs.com) · [Install](https://gaze.gundulabs.com/guide/installation) · [Development](https://gaze.gundulabs.com/guide/development)

</div>

---

> [!NOTE]
> Gaze includes local liveness anti-spoofing checks and supports infrared (IR) cameras. In high-security environments, we recommend keeping your usual system authentication enabled as a fallback.

Gaze brings facial authentication to Linux with on-device face recognition, PAM integration, and tools for login, lock screens, `sudo`, and desktop management.

## Install

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

The installer sets up the Gaze daemon, CLI, and GUI. On openSUSE Tumbleweed (x86_64), it uses the native `zypper` package manager and Gaze's Tumbleweed-specific RPM repository.

It chooses desktop integration based on your session: GNOME gets the GNOME Shell extension, Cinnamon gets `gaze-cinnamon-extension`, KDE Plasma gets `gaze-kde`, and Quickshell Omarchy gets `gaze-omarchy`. On other desktops, the installer skips desktop extensions rather than pulling in GNOME Shell.

If you installed the GNOME extension manually, or the installer could not enable it automatically, reboot first so GNOME Shell can scan the new extension. Then, from your GNOME session, run:

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

> If you run `gnome-extensions enable` before rebooting, it reports `Extension "gaze@gundulabs.com" does not exist`. GNOME Shell scans extension directories only when a session starts and drops IDs it has not scanned. As a result, enabling the extension before reboot can appear to work but disappear at your next logout. To avoid this, reboot before running the commands above. `gaze doctor` can detect the issue and prints the same steps.

<details>
<summary>Manual install (Debian/Ubuntu, Fedora/openSUSE RPM systems, Arch/Manjaro/CachyOS)</summary>

**Debian / Ubuntu**

Each apt suite carries only the builds for that release: `noble` (Ubuntu 24.04), `questing` (Ubuntu 25.10), `resolute` (Ubuntu 26.04), `stonking` (Ubuntu 26.10), `trixie` (Debian 13), `forky` (Debian 14, testing).

```bash
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.gpg \
  | sudo tee /usr/share/keyrings/gundulabs-archive-keyring.gpg >/dev/null
suite="$(. /etc/os-release && echo "${VERSION_CODENAME:-$UBUNTU_CODENAME}")"
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] https://packages.gundulabs.com/deb $suite main" \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui
```

**Fedora and compatible DNF systems**

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

**Fedora OSTree (Silverblue / Bazzite / Kinoite)**

```bash
sudo tee /etc/yum.repos.d/gundulabs.repo >/dev/null <<'EOF'
[gundulabs]
name=Gundu Labs
baseurl=https://packages.gundulabs.com/rpm/fedora/$releasever/$basearch
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://packages.gundulabs.com/keys/gundulabs-repo.asc
EOF
sudo rpm-ostree install gaze gaze-gui
```

**Fedora via Copr** (alternative to the repository above; do not enable both)

```bash
sudo dnf install dnf-plugins-core
sudo dnf copr enable gundulabs/gaze
sudo dnf install gaze gaze-gui
```

**openSUSE Tumbleweed (x86_64)**

```bash
sudo rpm --import https://packages.gundulabs.com/keys/gundulabs-repo.asc
sudo tee /etc/zypp/repos.d/gundulabs.repo >/dev/null <<'EOF'
[gundulabs]
name=Gundu Labs
baseurl=https://packages.gundulabs.com/rpm/opensuse/tumbleweed/$basearch
enabled=1
autorefresh=1
type=rpm-md
gpgcheck=1
gpgkey=https://packages.gundulabs.com/keys/gundulabs-repo.asc
EOF
sudo zypper refresh
sudo zypper install gaze gaze-gui
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

On openSUSE Tumbleweed, the RPM post-install script enables Gaze in the shared PAM stack. If needed, reapply that setting with `sudo pam-config --add --gaze && sudo pam-config --update`.

For GNOME lock screen face unlock after a manual package install, also install `gaze-gnome-extension` (`gaze-gnome-extension-bin` on Arch) and reboot. Then run `gnome-extensions enable gaze@gundulabs.com` and `gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true` from your GNOME session. On Cinnamon, install `gaze-cinnamon-extension` and enable it from **System Settings → Extensions**; see the [Cinnamon guide](https://gaze.gundulabs.com/guide/cinnamon). On KDE Plasma, install `gaze-kde` (`gaze-kde-bin` on Arch) to enable hands-free lock screen face unlock and add a Face Unlock entry to System Settings; see the [KDE guide](https://gaze.gundulabs.com/guide/kde).

</details>

<details>
<summary>Nix / NixOS (flake)</summary>

The repo is a Nix flake with packages (`gaze`, `gaze-gui`, `gaze-gnome-extension`, `gaze-cinnamon-extension`) and a NixOS module that configures the daemon, D-Bus/polkit, and PAM declaratively:

```nix
# flake.nix inputs
inputs.gaze.url = "github:GunduLabs/gaze";

# NixOS configuration
imports = [ inputs.gaze.nixosModules.default ];
services.gaze = {
  enable = true;
  gui.enable = true;
};
```

See the [Nix & NixOS guide](https://gaze.gundulabs.com/guide/nixos) for module options, GNOME lock screen setup, hyprlock, and home-manager usage.

</details>

After installing Gaze by any method, reboot once to apply all system-level changes.

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

Gaze's daemon (`gazed`) communicates over DBus. When PAM, the GNOME lock screen extension, or the CLI requests authentication, the daemon captures a frame from your webcam, detects and aligns your face, then uses an ONNX model to create an embedding and compare it with your enrolled profiles.

Everything stays on your machine: Gaze processes faces locally and stores embeddings on disk. It does not transmit face data anywhere.

```
Camera → Face Detection (SCRFD) → Alignment → Embedding (ArcFace) → Match → Liveness (MiniFASNet-V2)
```

## Components

| Component | Description |
|-----------|-------------|
| `gazed` | System daemon exposing `com.gundulabs.Gaze` on DBus |
| `gaze` | CLI for enrollment and authentication (crate: `gaze-cli`) |
| `gaze-gui` | GTK4/Adwaita graphical application |
| `pam-gaze` | PAM module for login/lock screen integration. Asks `gazed` over DBus; links no camera or inference code |
| `gaze-security` | TPM sealing and the privileged credential store behind template encryption and keyring unlock |
| `gaze-gnome-extension` | GNOME Shell extension for lock screen and GDM auth |
| `gaze-cinnamon-extension` | Cinnamon Spices extension for lock screen and PolKit auth |
| `gaze-kde` | KDE Plasma lock screen wiring and a Face Unlock entry in System Settings |
| `gaze-omarchy` | Omarchy Quickshell lock plugin with independent password, fingerprint and face authentication |
| `gaze-hyprlock` | PAM service for hyprlock face unlock on Hyprland |

## Configuration

```toml
# /etc/gaze/config.toml
[inference]
execution_provider = "cpu" # cpu | auto | openvino | vitis
device = "cpu"             # auto/vitis: npu; openvino: cpu | gpu | npu

[security]
level = "medium"    # low | medium | high | maximum | custom

[cameras]
rgb = "primary"
dark_luma_threshold = 20

[auth]
abort_if_ssh = true
abort_if_lid_closed = true

[enrollment]
max_templates = 2
min_face_size_ratio = 0.25

[liveness]
enabled = true
threshold = 0.8

[storage]
encrypt_templates = false   # seal face templates to the TPM
unlock_kwallet = false # optional TPM-backed KDE wallet unlock
unlock_gnome_keyring = false # unlock the GNOME keyring after a GDM or greetd face login
```

Standard builds support Intel OpenVINO and AMD Ryzen AI NPU runtimes, but use
the CPU by default. To enable acceleration, install the vendor drivers and
runtime, register it under `/usr/lib/gaze/runtimes`, and set `auto/npu`. Then
restart `gazed` and run `gaze doctor --benchmark`.
See the [hardware acceleration guide](https://gaze.gundulabs.com/guide/acceleration) for supported hardware,
SDK installation, CPU fallback, and precision validation.

See the [configuration guide](https://gaze.gundulabs.com/guide/configuration) for all options.

## CLI usage

```
gaze add-face <name>         Enroll a new face
gaze refine-face <name>      Add samples to an existing enrollment
gaze auth                    Authenticate
gaze auth --verbose          Authenticate with detailed metrics
gaze auth --silent           Authenticate silently (exit code only)
gaze list-faces              List enrolled faces
gaze rename-face <old> <new> Rename a face
gaze remove-face <name>      Remove a face
gaze clear-user              Remove all face data for current user
gaze config                  Interactive configuration editor
gaze config --show           Print current config and exit
gaze keyring                 Enroll optional TPM-backed GNOME Keyring unlock
gaze keyring --forget        Remove the stored GNOME Keyring credential
gaze keyring --kwallet       Enroll optional TPM-backed KDE KWallet unlock
gaze doctor                  Check config, daemon, cameras, enrollments, PAM, and TPM
gaze doctor --benchmark      Also measure detector/recognizer/liveness inference speed
gaze uninstall               Completely remove Gaze (packages, PAM, config, models, data)
gaze uninstall -y            Skip confirmation prompt
```

Enrollment starts with a straight-on reference. Gaze then asks you to make small
up, down, left, and right movements relative to that pose.

## Building from source

**Dependencies:** Rust 1.85+, [`just` 1.51+](https://github.com/casey/just), [`nfpm`](https://nfpm.goreleaser.com)

```bash
# Ubuntu/Debian
sudo apt install build-essential pkg-config clang libclang-dev \
  libopencv-dev libv4l-dev libpam0g-dev libtss2-dev libssl-dev \
  libgtk-4-dev libadwaita-1-dev \
  libcairo2-dev libglib2.0-dev libgdk-pixbuf-2.0-dev libpango1.0-dev libgraphene-1.0-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-pipewire \
  gettext-base

# Build
just build-rust

# The same build includes Intel and AMD NPU provider adapters
# Vendor drivers and runtimes are installed separately; see the hardware acceleration guide

# Package
just package <deb | rpm | archlinux>
```

See the [development guide](https://gaze.gundulabs.com/guide/development) for more.

## Repository layout

| Directory | Contents |
| --- | --- |
| `crates/` | Rust daemon, CLI, GUI, shared libraries, and PAM modules |
| `integrations/` | GNOME Shell, Cinnamon, KDE, and Omarchy source |
| `packaging/` | Package definitions, lifecycle hooks, and installed system configuration |
| `scripts/` | Development helpers and integration test harnesses |
| `docs/` | User and contributor documentation |

The root `Cargo.toml`, `Justfile`, and `flake.nix` coordinate the workspace.

## License

Gaze is free software licensed under the [GNU General Public License, version 3 or later](LICENSE) (`GPL-3.0-or-later`).

```
Gaze - Facial authentication for Linux
Copyright (C) 2026 Gundu Labs <maintainers@gundulabs.com>

This program is free software: you can redistribute it and/or modify it under
the terms of the GNU General Public License as published by the Free Software
Foundation, either version 3 of the License, or (at your option) any later
version.

This program is distributed in the hope that it will be useful, but WITHOUT ANY
WARRANTY; without even the implied warranty of MERCHANTABILITY or FITNESS FOR A
PARTICULAR PURPOSE. See the GNU General Public License for more details.

You should have received a copy of the GNU General Public License along with
this program. If not, see <https://www.gnu.org/licenses/>.
```

Contributions are accepted under the same license.
