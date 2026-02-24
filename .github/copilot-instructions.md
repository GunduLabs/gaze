# Copilot instructions for the Gaze repository

Purpose
- Short, focused guidance for future Copilot/Copilot-CLI sessions working on this repo.

Build, test, and lint commands
- Build entire workspace (release):
  - cargo build --workspace --release
- Build single binaries / packages (release):
  - Daemon: cargo build --bin gazed --release
  - CLI:    cargo build --bin gaze --release
  - GUI:    cargo build -p gaze_gui --release
  - PAM:    cargo build -p pam_gaze --release
- Run full test suite (release):
  - cargo test --workspace --release
- Run a single test (examples):
  - From workspace: cargo test <test_name> -p <package> --release
    - Example: cargo test auth_success -p gaze --release
  - From inside a crate: cd gaze_common && cargo test <test_name>
  - Run all tests for a single package: cargo test -p gaze_common
- Lint & format (common Rust tools; project does not mandate these but they are useful):
  - cargo clippy --workspace -- -D warnings
  - cargo fmt --all -- --check

High-level architecture (big picture)
- Purpose: Rust-based facial-authentication daemon for Linux using InsightFace ONNX models; integrates with PAM and exposes a DBus API (org.gaze.Auth).
- Workspace crates (root Cargo.toml):
  - gaze (root) — contains daemon binary (gazed) and CLI (gaze). Core ML pipeline and main orchestration live here (src/ and src/daemon/).
  - gaze_common — shared library with camera capture wrappers, DBus proxies, config parsing, and centering logic.
  - gaze_gui — GTK4/Adwaita GUI for enrollment/auth flows.
  - pam_gaze — PAM module (cdylib) exposing C FFI entry points for system auth.
- Data flow (concise): Camera frame (OpenCV) → DBus → Daemon: SCRFD detection → Umeyama alignment (112×112) → ResNet50/MobileFaceNet embedding → cosine similarity vs stored embeddings → authentication result.
- Runtime & IPC:
  - Daemon runs as an async Tokio service and registers on DBus as `org.gaze.Auth` at `/org/gaze/Auth` (see src/main.rs and src/daemon.rs).
  - CLI (src/bin/cli.rs) uses gaze_common DBus proxy to communicate with the daemon and provides subcommands: auth, add-face, refine-face, remove-face, clear-user.
- Important paths and artifacts:
  - Default config template: dist/config.toml → system config at /etc/gaze/config.toml
  - Models auto-downloaded to /opt/gaze/models/
  - User embeddings stored under: /var/lib/gaze/users/{username}/{face_name}/{uuid}.bin
  - Distribution files: dist/ (systemd service, DBus policy, packaging helpers)

Key conventions and repo-specific patterns
- Error handling: use anyhow::Result widely across crates.
- Async & IPC: tokio runtime for async I/O and zbus derive macros for DBus interfaces (gaze_common/src/dbus.rs).
- ML pipeline modularization: detector → aligner → recognizer modules (see src/daemon/{recognize.rs,align.rs,models.rs,users.rs}). Alignments use an Umeyama transform to produce ArcFace-style 112×112 inputs.
- Models: models.rs contains logic to download InsightFace ONNX artifacts from GitHub releases on demand — be mindful of network calls in CI or tests.
- PAM module: pam_gaze is a cdylib with unsafe C FFI exported functions; changes here require careful review and testing on target systems.
- Testing note: the repository's README/CLAUDE.md lists tests run in release mode; when debugging locally, prefer debug builds but be aware behavioral differences.
- When changing DBus interfaces, update gaze_common proxy/interface definitions and corresponding zbus derives.

Repository docs & AI-config files
- Primary internal doc used to build this guidance: CLAUDE.md (contains project overview and build/test commands).
- No other AI assistant config files (e.g., .cursorrules, AGENTS.md, .windsurfrules) were found in the repo at the time this file was created.

Quick references
- Binaries: gazed (daemon), gaze (CLI)
- CLI usage: ensure gazed is running (or use the systemd service from dist/) then run the CLI (target/release/gaze auth ...)

If you want additional areas covered (examples: developer workflow for debugging the PAM module, CI test matrix details, or expanded single-test examples), mention which area to expand.
