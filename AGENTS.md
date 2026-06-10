# AGENTS.md

Gaze is facial authentication for Linux. A root daemon (`gazed`) runs the ML
pipeline and owns a system-DBus interface; the PAM modules, CLI, and GTK GUI are
all clients of it.

## Workspace

- `gaze` — both binaries: the `gazed` daemon (`src/main.rs`) and the `gaze` CLI
  (`src/bin/cli.rs`), plus the ML pipeline and user database.
- `gaze-core` — shared camera/config/DBus/detection library. DBus proxy and
  types are generated from `src/dbus.rs` with `zbus` macros.
- `pam-gaze`, `pam-gaze-grosshack` — `cdylib` PAM modules; shared FFI/auth logic
  lives in `pam-gaze-core`.
- `gaze-gui` — GTK4/libadwaita app. `gnome-shell-extension/` ships separately.
- Root `default-members` omit the two `*-core` libraries, so use `--workspace`
  for whole-repo checks.

## Commands

- Build: `just build-rust` (`cargo build --workspace --release`).
- Test: `just test` (`cargo test --workspace --release`); focused:
  `cargo test -p <crate> --release <name>`.
- Lint/format: `just lint` (`clippy --workspace --all-targets -- -D warnings`),
  `just fmt-check`, `just --fmt --check`.
- CI runs, in order: fmt checks → `just lint` → `just test` → `just audit` →
  `just build-rust`.
- Package: `just package <deb|rpm|archlinux>`.
- Docs (VitePress under `docs/`): use **bun** — `bun run docs:dev`,
  `bun run docs:build`. Never create `package-lock.json`.
- Native builds need OpenCV, clang/libclang, v4l, PAM, GTK4/libadwaita, and
  GStreamer dev packages (see `.github/workflows/ci.yml` for exact names).

## Runtime

- Fixed paths, no CLI override: config `/etc/gaze/config.toml`, templates
  `/var/lib/gaze/users`, models `/var/cache/gaze`.
- `gazed` runs as root and owns `com.gundulabs.Gaze` at `/com/gundulabs/Gaze` on
  the system bus. CLI, GUI, and PAM all talk to whichever daemon owns that name.
- Local loop: `sudo systemctl stop gazed`, build, then
  `sudo RUST_LOG=debug ./target/release/gazed`; restart the service when done.
- Models download from InsightFace/HuggingFace on first run and are SHA-256
  verified. Tests must not depend on the network, models, or a physical camera.
- Camera config is `primary` or a GStreamer/PipeWire source string; raw
  `/dev/video*` paths are rejected.

## Safety

- PAM changes can lock users out: keep a root shell open, test a non-critical
  service first, and record manual verification steps.
- Never commit downloaded models, face embeddings, local `/etc/gaze`, or
  artifacts under `dist/`, `target/`, `node_modules/`, or `docs/.vitepress/dist`.
- Update `docs/` and `README.md` when CLI, config, install, packaging, or auth
  behavior changes.
