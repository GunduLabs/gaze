# Getting Started

Gaze is a facial authentication daemon for Linux. It uses InsightFace ONNX models (SCRFD detection + ArcFace recognition) to provide face-based login via PAM and a DBus interface.

## Requirements

**Rust toolchain** (2024 edition) and the following system libraries:

::: code-group

```sh [Debian/Ubuntu]
sudo apt install libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev
```

```sh [Fedora/RHEL]
sudo dnf install opencv-devel clang-devel libv4l-devel pam-devel gtk4-devel libadwaita-devel
```

:::

## Building from source

```bash
cargo build --workspace --release
```

Or build individual components:

```bash
cargo build --bin gazed --release        # Daemon only
cargo build --bin gaze --release         # CLI only
cargo build -p gaze_gui --release        # GTK4 GUI
cargo build -p pam_gaze --release        # PAM module (libpam_gaze.so)
```

## Quick install

See the [Installation guide](./installation) for full setup instructions including systemd, DBus policy, PAM config, and GNOME Shell extension.
