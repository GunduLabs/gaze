# AGENTS.md

Guidance for AI coding agents working in this repo. Keep it short; update as the project changes.

## What Gaze is

A Rust facial authentication daemon for Linux. Pipeline: camera frame → SCRFD detection → Umeyama alignment (112×112) → ArcFace embedding (ResNet50 or MobileFaceNet, ONNX) → cosine similarity vs. stored templates.

The daemon (`gazed`) runs as a system service and exposes `com.gundulabs.Gaze` on the DBus system bus at `/com/gundulabs/Gaze`. Clients (CLI, GUI, PAM module, GNOME extension) talk to it over DBus.

## Workspace

Rust 2024, resolver v3. Six crates:

- `gaze` — daemon (`gazed`) + CLI (`gaze`). ML pipeline lives here (`align.rs`, `recognize.rs`, `models.rs`, `users.rs`, `daemon.rs`).
- `gaze-core` — shared lib: camera, detection wrapper, capture session, config, DBus proxy/types.
- `gaze-gui` — GTK4/Adwaita app.
- `pam-gaze` — PAM module (`cdylib`, `libpam_gaze.so`). Thin FFI wrapper.
- `pam-gaze-core` — PAM auth logic shared by the module.
- `pam-gaze-grosshack` — PAM compatibility shim (simultaneous mode).

GNOME Shell extension lives in `gnome-shell-extension/` (packaged separately as `gaze-gnome-extension`).

## Build & test

```bash
cargo build --workspace --release
cargo test --workspace --release
just lint        # clippy -D warnings
just fmt-check
just package deb | rpm | archlinux   # via nfpm
```

System deps (Debian/Ubuntu): `libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev`.

## Key paths

- Config: `/etc/gaze/config.toml` (template: `packaging/config/config.toml`)
- User templates: `/var/lib/gaze/users/{username}/{face_name}/{uuid}.bin`
- Models: `/var/cache/gaze/` — auto-downloaded from InsightFace GitHub releases on first run; never commit them.
- Packaging sources: `packaging/` (systemd, DBus policy, PAM/authselect/GDM/SELinux, nfpm manifests).
- Built packages: `dist/packages/`.

## Config shape

`/etc/gaze/config.toml` is TOML with `[security]`, `[cameras]`, `[enrollment]` sections. `security.level` is `low | medium | high | maximum | custom`. Camera source is a GStreamer pipeline string (e.g. `"primary"`, `"pipewiresrc target-object=…"`); `/dev/video*` paths are not supported.

## Conventions

- Errors: `anyhow::Result` end-to-end.
- Async: Tokio for all IPC and I/O.
- DBus interfaces: `zbus` derive macros in `gaze-core/src/dbus.rs`.
- PAM crates touch unsafe C FFI — review changes carefully.
- Don't bundle ML models in the repo.

## CLI subcommands

`auth`, `add-face`, `refine-face`, `list-faces`, `rename-face`, `remove-face`, `clear-user`, `config`. All talk to `gazed` over DBus.

## Docs

User-facing docs are in `docs/` (VitePress, published at https://gaze.gundulabs.com). README.md is the GitHub landing page. Update both when user-visible behavior changes.
