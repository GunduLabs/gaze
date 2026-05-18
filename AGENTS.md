# AGENTS.md

## Repo Shape

- Rust workspace members are `gaze`, `gaze-core`, `pam-gaze-core`, `pam-gaze`, `pam-gaze-grosshack`, and `gaze-gui`; root `default-members` omit the two `*-core` libraries, so use `--workspace` for whole-repo checks.
- `gaze` owns both binaries: `gazed` at `gaze/src/main.rs` and CLI `gaze` at `gaze/src/bin/cli.rs`; the ML pipeline/user DB code also lives in this crate.
- `gaze-core` is the shared camera/config/DBus/detection library; DBus proxy/types are generated from `gaze-core/src/dbus.rs` with `zbus` macros.
- `pam-gaze` and `pam-gaze-grosshack` are `cdylib` PAM modules; shared PAM FFI/auth logic is in `pam-gaze-core`.
- `gaze-gui` is the GTK4/libadwaita app; `gnome-shell-extension/` is packaged separately as `gaze-gnome-extension`.

## Commands

- CI uses `just --fmt --check`, `just fmt-check`, `just lint`, `just test`, `just audit`, then `just build-rust`.
- `just lint` is `cargo clippy --workspace --all-targets -- -D warnings`; the local pre-commit hook is narrower, so do not treat it as CI-equivalent.
- `just test` is `cargo test --workspace --release`; focused equivalent: `cargo test -p <crate> --release <test_name>`.
- `just build-rust` is `cargo build --workspace --release`.
- Native builds need OpenCV, clang/libclang, v4l, PAM, GTK4/libadwaita, and GStreamer dev packages; use `.github/workflows/ci.yml` for exact distro package names.
- `just package <deb|rpm|archlinux>` builds Rust, SELinux policy, then all three nfpm packages into `dist/packages/`.
- `just package-prebuilt <deb|rpm|archlinux>` assumes `target/release/*` already exists; RPM GNOME-extension packaging also needs `dist/selinux/gaze-gdm-camera.pp` from SELinux tools.
- Docs are VitePress under `docs/`: `bun run docs:dev`, `bun run docs:build`, `bun run docs:preview`. `bun.lock` is the tracked JS lockfile; do not create `package-lock.json`.

## Runtime Gotchas

- `gazed` has fixed runtime paths: config `/etc/gaze/config.toml`, templates `/var/lib/gaze/users`, models `/var/cache/gaze`; there is no CLI flag for alternate paths.
- The daemon owns `com.gundulabs.Gaze` on the system DBus bus at `/com/gundulabs/Gaze`; run it as root and stop the installed `gazed` service before foreground local testing.
- Local daemon loop from docs: `sudo systemctl stop gazed`, `cargo build --workspace --release`, `sudo RUST_LOG=debug ./target/release/gazed`, then restart the service when done.
- CLI, GUI, and PAM clients always talk to whichever daemon owns the system bus name.
- Models are downloaded from InsightFace releases on first daemon run if absent; tests should not depend on network or committed model files.
- Camera config is `primary` or a GStreamer/PipeWire source string such as `pipewiresrc target-object=...`; `/dev/video*` paths are rejected in `gaze-core/src/camera.rs`.

## Testing And Safety

- Prefer CI-safe tests around config, DBus mapping, user DB, model helpers without downloads, alignment/math, and CLI/TUI helpers.
- Do not add automated tests requiring a physical camera, running system `gazed`, PAM installed into system auth files, a graphical session, or network downloads.
- PAM changes can lock users out: keep an active root shell, test a non-critical PAM service first, and include manual verification notes.
- Do not commit downloaded ONNX models, face embeddings, local `/etc/gaze` config, package artifacts under `dist/`, `target/`, `node_modules/`, or `docs/.vitepress/dist`.

## Packaging And Extension Notes

- Main package manifest is `packaging/nfpm.yaml`; GUI and GNOME extension manifests are `packaging/nfpm-gui.yaml` and `packaging/nfpm-gnome-extension.yaml`.
- DBus policy, polkit policy, systemd unit, default config, GSettings schema, PAM/GDM/authselect files, and SELinux policy all live under `packaging/`.
- The GNOME extension source directory currently has only `extension.js`, `prefs.js`, and `metadata.json`; its GSettings schema source is `packaging/config/org.gnome.shell.extensions.gaze.gschema.xml`.
- User-facing docs live in `docs/`; `README.md` is the GitHub landing page. Update docs when CLI, config, install, packaging, or auth behavior changes.
