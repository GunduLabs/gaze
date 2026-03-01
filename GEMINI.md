# GEMINI.md

This file provides architectural context and development guidelines for Gaze, a Linux facial authentication system.

## Project Overview

Gaze is a Rust-based facial authentication daemon for Linux. It provides a secure and efficient way to authenticate users using biometric data (face recognition) via InsightFace ONNX models. The system is designed to integrate deeply with the Linux ecosystem through PAM and DBus.

### Core Components

- **`gazed` (Daemon)**: The central service (`org.gaze.Auth`) that handles ML inference (detection, alignment, recognition) and manages the user database.
- **`gaze` (CLI)**: A command-line tool for management (enrollment, authentication tests, etc.).
- **`pam_gaze`**: A PAM (Pluggable Authentication Module) implementation that enables Gaze for system-wide login, sudo, and lock screens. Thin wrapper over `pam_gaze_core`.
- **`pam_gaze_core`**: Core PAM authentication logic shared by the PAM module.
- **`pam_gaze_grosshack`**: PAM compatibility shim for environments that require it.
- **`gaze_gui`**: A GTK4/Adwaita application for user-friendly face enrollment and management.
- **`gaze_core`**: Shared logic for camera interaction, capture sessions, face checking, configuration, and DBus communication.

## Architecture & Data Flow

1. **Capture**: Frames are captured via OpenCV (`gaze_core/src/camera.rs`).
2. **Detection**: SCRFD (Sample and Computation Redistribution for Efficient Face Detection) is used to locate faces.
3. **Alignment**: Faces are aligned using the Umeyama transform to a standard 112x112 size.
4. **Embedding**: A ResNet50 or MobileFaceNet model generates a 512-dimensional embedding.
5. **Comparison**: Cosine similarity is used to compare the live embedding against stored templates in `/var/lib/gaze/users/`.
6. **IPC**: Communication between components happens over the DBus System Bus.

## Technical Stack

- **Language**: Rust (2024 Edition)
- **ML Runtime**: ONNX Runtime (`ort`)
- **Image Processing**: OpenCV, `image`, `ndarray`
- **IPC**: `zbus` (DBus)
- **GUI**: GTK4, Libadwaita
- **Async**: Tokio
- **Error Handling**: `anyhow`

## Development Workflow

### Prerequisites (Ubuntu/Debian)
```bash
sudo apt install libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev
```

### Build Commands
- **Build all components**: `cargo build --workspace --release`
- **Build specific binary**: `cargo build --bin gazed` or `cargo build --bin gaze`
- **Build PAM module**: `cargo build -p pam_gaze --release` (produces `libpam_gaze.so`)
- **Build GUI**: `cargo build -p gaze_gui --release`
- **Run Tests**: `cargo test --workspace`

### Testing the PAM Module
Testing PAM requires caution. It is recommended to test the PAM module by adding it to a specific service like `sudo` or a custom PAM service before applying it to `common-auth`.
The module expects to be located in `/lib/x86_64-linux-gnu/security/pam_gaze.so` (on Debian/Ubuntu).

## Key File Locations

- **System Config**: `/etc/gaze/config.toml`
- **User Templates**: `/var/lib/gaze/users/{username}/`
- **ML Models**: `/opt/gaze/models/`
- **Systemd Service**: `gazed.service`
- **DBus Policy**: `/etc/dbus-1/system.d/org.gaze.Auth.conf`

## Coding Conventions

- **Safety**: `pam_gaze` uses unsafe C FFI. Any changes there must be rigorously audited for memory safety.
- **Errors**: Use `anyhow::Result` for application-level logic.
- **Async**: Prefer Tokio's async primitives for I/O and IPC.
- **Models**: Do not bundle models in the repository. The daemon will download them on first run if they are missing from `/opt/gaze/models/`.
- **Formatting**: Adhere to standard `cargo fmt`.
