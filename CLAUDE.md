# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Gaze is a Rust-based facial authentication daemon for Linux. It uses InsightFace ONNX models for face detection, alignment, and recognition. The system integrates with PAM for system login and exposes a DBus interface (`org.gaze.Auth`) for IPC. A GNOME Shell extension enables lock-screen face auth via GDM.

## Build Commands

```bash
cargo build --workspace --release       # Build everything
cargo build --bin gazed --release       # Daemon only
cargo build --bin gaze --release        # CLI only
cargo build -p gaze-gui --release       # GTK4 GUI
cargo build -p pam-gaze --release       # PAM module (libpam_gaze.so)
cargo test --workspace --release        # Run all tests
```

System dependencies (Ubuntu): `libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev`

## Workspace Structure

Six crates in a Cargo workspace (Rust 2024 edition, resolver v3):

- **`gaze`** - Daemon (`gazed`) and CLI (`gaze`) binaries. Contains the core ML pipeline: face detection → alignment → recognition → embedding comparison.
- **`gaze-core`** - Shared library used by all other crates. Camera capture, face detection wrapper, capture session logic, configuration parsing, DBus proxy definitions.
- **`gaze-gui`** - GTK4/Adwaita GUI application for enrollment and authentication.
- **`pam-gaze`** - PAM module (`cdylib`). Thin wrapper that calls into `pam-gaze-core`.
- **`pam-gaze-core`** - Core PAM authentication logic shared by the PAM module.
- **`pam-gaze-grosshack`** - PAM compatibility shim for environments that require it.

The `gnome-shell-extension/` directory contains the GNOME Shell extension (`gaze@gundulabs.com`) packaged separately.

## Architecture

**Data flow**: Camera frame (OpenCV) → DBus → Daemon: SCRFD detection → Umeyama alignment (112×112) → ResNet50/MobileFaceNet embedding → cosine similarity against stored embeddings → auth result.

**Daemon (`gaze/src/main.rs`, `gaze/src/daemon.rs`)**: Async Tokio service registered on DBus as `org.gaze.Auth` at `/org/gaze/Auth`. The `AuthDaemon` struct holds the detector, recognizer, and user database. Key daemon modules in `gaze/src/`:
- `align.rs` - Umeyama transform for ArcFace-standard face alignment
- `recognize.rs` - ONNX inference for face embeddings
- `models.rs` - Downloads InsightFace models from GitHub releases on demand
- `users.rs` - File-based embedding database at `/var/lib/gaze/users/{username}/{face_name}/{uuid}.bin`

**CLI (`gaze/src/bin/cli.rs`)**: Clap-based tool with subcommands: `auth`, `add-face`, `refine-face`, `list-faces`, `remove-face`, `clear-user`. Communicates with daemon via DBus proxy from `gaze-core`.

**Configuration**: TOML at `/etc/gaze/config.toml` with security levels (low/medium/high/maximum/custom). Default config template in `dist/config.toml`.

## Key Conventions

- Error handling with `anyhow::Result` throughout
- Async/await with Tokio for all IPC and I/O
- DBus interface defined via `zbus` derive macros in `gaze-core/src/dbus.rs`
- ML models auto-downloaded to `/opt/gaze/models/` on first run
- PAM module uses unsafe C FFI - changes require careful review

## Distribution Files

`dist/` contains system integration files: systemd service (`gazed.service`), DBus policy (`org.gaze.Auth.conf`), default config. CD packages these into deb/rpm/arch packages via `.github/workflows/cd.yml`.
