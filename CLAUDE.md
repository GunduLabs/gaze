# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Gaze is a Rust-based facial authentication daemon for Linux. It uses InsightFace ONNX models for face detection, alignment, and recognition. The system integrates with PAM for system login and exposes a DBus interface (`org.gaze.Auth`) for IPC.

## Build Commands

```bash
cargo build --workspace --release       # Build everything
cargo build --bin gazed --release       # Daemon only
cargo build --bin gaze --release        # CLI only
cargo build -p gaze_gui --release       # GTK4 GUI
cargo build -p pam_gaze --release       # PAM module (libpam_gaze.so)
cargo test --workspace --release        # Run all tests
```

System dependencies (Ubuntu): `libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev`

## Workspace Structure

Four crates in a Cargo workspace (Rust 2024 edition, resolver v3):

- **`gaze`** (root) — Daemon (`gazed`) and CLI (`gaze`) binaries. Contains the core ML pipeline: face detection → alignment → recognition → embedding comparison.
- **`gaze_common`** — Shared library used by all other crates. Camera capture, face detection wrapper, configuration parsing, DBus proxy definitions, centering logic.
- **`gaze_gui`** — GTK4/Adwaita GUI application for enrollment and authentication.
- **`pam_gaze`** — PAM module (`cdylib`). Exports C FFI functions (`pam_sm_authenticate`, `pam_sm_setcred`, `pam_sm_acct_mgmt`).

## Architecture

**Data flow**: Camera frame (OpenCV) → DBus → Daemon: SCRFD detection → Umeyama alignment (112×112) → ResNet50/MobileFaceNet embedding → cosine similarity against stored embeddings → auth result.

**Daemon (`src/main.rs`, `src/daemon.rs`)**: Async Tokio service registered on DBus as `org.gaze.Auth` at `/org/gaze/Auth`. The `AuthDaemon` struct holds the detector, recognizer, and user database. Key daemon modules in `src/daemon/`:
- `align.rs` — Umeyama transform for ArcFace-standard face alignment
- `recognize.rs` — ONNX inference for face embeddings
- `models.rs` — Downloads InsightFace models from GitHub releases on demand
- `users.rs` — File-based embedding database at `/var/lib/gaze/users/{username}/{face_name}/{uuid}.bin`

**CLI (`src/bin/cli.rs`)**: Clap-based tool with subcommands: `auth`, `add-face`, `refine-face`, `remove-face`, `clear-user`. Communicates with daemon via DBus proxy from `gaze_common`.

**Configuration**: TOML at `/etc/gaze/config.toml` with security levels (low/medium/high/maximum/custom). Default config template in `dist/config.toml`.

## Key Conventions

- Error handling with `anyhow::Result` throughout
- Async/await with Tokio for all IPC and I/O
- DBus interface defined via `zbus` derive macros in `gaze_common/src/dbus.rs`
- ML models auto-downloaded to `/opt/gaze/models/` on first run
- PAM module uses unsafe C FFI — changes require careful review

## Distribution Files

`dist/` contains system integration files: systemd service (`gazed.service`), DBus policy (`org.gaze.Auth.conf`), default config. CI packages these into deb/rpm/arch packages via `.github/workflows/build.yml`.
