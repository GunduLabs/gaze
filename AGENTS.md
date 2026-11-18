# AGENTS.md

This file provides guidance to AI coding agents working with code in this repository.

Gaze is facial authentication for Linux: a root daemon (`gazed`) on the system DBus, a CLI/TUI, a GTK4 GUI, PAM modules, and desktop lock screen integrations. It is authentication software, so a bug can lock someone out or let the wrong person in.

For anything not covered here, use the documentation at https://gaze.gundulabs.com. Its source is `docs/src/guide/`, so a page like https://gaze.gundulabs.com/guide/development is `docs/src/guide/development.md` in this checkout. Start with `development.md` (environments, workflows, packaging), `contributing.md` (PR expectations, testing, PAM safety), `configuration.md`, `pam.md`, `acceleration.md`, and `troubleshooting.md`.

## Host platform

Everything targets Linux APIs (V4L2, PAM, tpm2-tss, polkit, GTK4, SELinux). Nothing builds natively on macOS. From a non-Linux host, run any recipe inside the Ubuntu build container: `just docker build-rust`, `just docker test`, `just docker package deb`. RPM and Arch packages need a Fedora/openSUSE or Arch environment instead. On Linux, `nix develop` gives a complete Rust dev shell.

## Commands

All work goes through `just` from the repo root (`just --list` shows every recipe).

```bash
just build-rust        # release build of all binaries + PAM link check
just test              # cargo test --workspace --release, plus keyring/kwallet PAM harness scripts
just lint              # clippy --workspace --all-targets -D warnings
just fmt-check         # rustfmt check (just fmt applies)
just audit             # cargo audit
just --fmt --check     # required if the Justfile changed
just check-pam-link    # verify PAM modules' shared-library allowlist
just package <deb|rpm|archlinux>   # required check for packaging/scripts/units/DBus/PAM changes
just build-docs        # VitePress via bun; `bun run docs:dev` for live preview
```

Running a single test directly with cargo:

```bash
cargo test --release -p gaze-core config::
cargo test --release -p gaze-vision inference:: -- --include-ignored
```

Notes for direct cargo use:
- Use `--release`: `just build-rust`/`just test` stage the pinned ONNX Runtime into `target/release/libonnxruntime.so`, which the inference tests load. Run one of those recipes once first.
- On distros with OpenCV 5 (e.g. Arch), set `OPENCV_PKGCONFIG_NAME=opencv5`; the just recipes do this automatically.
- `GAZE_GUI=0` drops `gaze-gui` (and gtk4/libadwaita) from `build-rust`, `test`, and `lint` only, not from bare cargo or packaging.

JS integration tests (Node 22+, downloaded upstream sources, simulated shell/PAM): `node scripts/test-gnome.mjs`, `node scripts/test-kde.mjs`, `node scripts/test-kcm.mjs`, `node scripts/test-cinnamon.mjs`.

CI (`.github/workflows/ci.yml`) runs fmt, audit, lint, test, `test-openvino`, the JS integration tests, and package builds. The pre-commit hook (`just setup-hooks`) runs `fmt-check` and `lint`.

## Architecture

Pipeline inside `gazed`: camera frame → SCRFD detection → alignment → ArcFace embedding → match against enrolled templates → MiniFASNet-V2 liveness. All clients (CLI, GUI, PAM, GNOME/Cinnamon/KDE/Omarchy integrations) talk to `gazed` over DBus (`com.gundulabs.Gaze`, system bus); none of them run inference.

Crates (`crates/`):
- `gazed`: daemon. DBus interface and auth flow in `daemon/` (`interface.rs`, `verify.rs`, `access.rs`), ML in `recognize.rs`/`align.rs`/`liveness.rs`/`models.rs`, user template DB in `users/`, TPM template encryption in `tpm/` and `crypto.rs`. Takes no CLI args; paths are compiled in (`/etc/gaze/config.toml`, `/var/lib/gaze/users`, `/var/cache/gaze`).
- `gaze-core`: shared config types, DBus proxy and DBus/config conversions (`dbus.rs`), IR emitter control (`ir/`), polkit, CPU checks. Deliberately light (no OpenCV/GStreamer/ORT) so PAM can depend on it.
- `gaze-security`: TPM sealing and the privileged credential store for keyring/KWallet unlock. Only tpm2-tss plus pure-Rust crypto, for the same reason.
- `gaze-vision`: camera capture, detection, inference runtime. Detection is behind the `detection` feature; the workspace dependency disables default features so only `gazed` enables it.
- `gaze-cli`: the `gaze` binary (CLI, TUI config, `setup`, `doctor/` diagnostics, `uninstall`).
- `gaze-gui`: GTK4/libadwaita app.
- `pam-gaze`: `cdylib` PAM module; asks `gazed` over DBus. `PamMode` is `Sequential`/`Simultaneous`/`Retry`.
- `pam-gaze-grosshack`: deprecated shim that `#[path]`-includes `pam-gaze` sources and forces `Simultaneous`. New work goes in `pam-gaze`.

Other trees: `integrations/` (GNOME Shell, Cinnamon, KDE KCM + `gaze-kde-pam`, Omarchy Quickshell), `packaging/` (nfpm, RPM spec, authselect, PAM configs, SELinux, systemd/DBus/polkit config, flatpak, nix, maintainer scriptlets), `scripts/` (dev helpers and test harnesses), `docs/src/` (VitePress site; `docs/archive/` and `docs/.vitepress/dist` are generated).

## Hard constraints

- **Never give the PAM modules a `gaze-vision` (or OpenCV/ORT) dependency, directly or through `gaze-core`/`gaze-security`.** `pam_gaze.so` is dlopened into every authenticating process including `sshd` and mail servers; OpenCV pulls in OpenBLAS, whose constructor aborts address-space-capped services. A crate boundary, not a feature flag, enforces this. `check-pam-link` allows only libpam, libc, libgcc, libm, the loader, and (main module only) `libtss2-{esys,mu,tctildr}`.
- **Build with `just build-rust`, not `cargo build --workspace`.** It builds `gazed` and the clients in separate cargo invocations so feature unification cannot link ONNX Runtime into clients or PAM.
- **Keep the `api-21` feature on `ort`.** Newer API levels either panic against older system runtimes (down to ORT 1.21) or abort in ORT's automatic EP selection on Linux. Don't drop it on an `ort` upgrade.
- Declare dependency versions only in root `[workspace.dependencies]`; crates use `name.workspace = true` and add only the features they need.
- Every new source file starts with the SPDX header (`// SPDX-FileCopyrightText: 2026 Gundu Labs` / `// SPDX-License-Identifier: GPL-3.0-or-later`; `#` comments below the shebang for scripts). New crates use `license.workspace = true`. Dependencies must be GPL-3.0-or-later compatible.
- CI tests must not need a camera, a running system `gazed`, installed PAM, a graphical session, or network model downloads. Favor tests for config parsing, DBus conversions, user DB logic, alignment/preprocessing math, and CLI helpers.
- Update `docs/src/guide/` when user-visible behavior, CLI output, config keys, install steps, or packaging change. Never edit `docs/.vitepress/dist`.

## Commits and branches

Commits follow [Conventional Commits v1.0.0](https://www.conventionalcommits.org/en/v1.0.0/):

```
<type>[optional scope][!]: <description>

[optional body]

[optional footer(s)]
```

- `fix` for bug fixes (SemVer PATCH), `feat` for new features (MINOR). Other types used here: `perf`, `refactor`, `style`, `test`, `docs`, `build`, `ops` (CI/CD and infrastructure, in place of `ci`), `chore`.
- Scope is a noun in parentheses naming the area touched. Common scopes in this repo: `gazed`, `cli`, `gui`, `pam`, `kde`, `gnome`, `camera`, `config`, `doctor`, `packaging`, `deps`.
- Description follows `: ` directly; imperative, lowercase first letter, no trailing period.
- Breaking changes: `!` before the colon (`feat(pam)!: drop grosshack shim`) and/or a `BREAKING CHANGE: <description>` footer (uppercase; `BREAKING-CHANGE` is a synonym).
- Footers are `Token: value` or `Token #value`, with `-` in place of spaces in the token (`Refs: #123`).
- `chore(deps)`/`fix(deps)` commits come from Renovate. Version bumps use `chore: release vX.Y.Z` and touch `Cargo.toml`, `Cargo.lock`, `integrations/omarchy/manifest.json`, and `packaging/gui/com.gundulabs.Gaze.metainfo.xml`.

Branches follow [Conventional Branch](https://conventionalbranch.org/): `<type>/<description>`.

- Types: `feature/` (or `feat/`), `bugfix/` (or `fix/`), `hotfix/` (urgent fixes), `release/` (e.g. `release/v0.6.0`), `chore/` (non-code work such as deps or docs). Branches created by an AI agent may instead use `ai/` or the agent's own prefix (`claude/`, `codex/`, `copilot/`, `cursor/`). `main` takes no prefix.
- Description: lowercase `a-z`, digits, hyphens between words, and dots only in versions. No uppercase, underscores, spaces, or other symbols. No consecutive, leading, or trailing hyphens or dots, and no hyphen next to a dot.
- Include a ticket number in lowercase when there is one: `fix/issue-123-locked-keyring`.

## Pull requests

When filling out `.github/pull_request_template.md`, only tick boxes that describe the change itself (type of change, areas touched, tests added). Leave unticked every box that attests to something a human did or agrees to, for the author to tick themselves:

- **PAM safety**: having a root shell open, testing on a non-critical service, checking password fallback.
- **Licensing**: agreeing to license the contribution under GPL-3.0-or-later. Checking each new dependency's license yourself.
- **AI disclosure**: never select "None". The confirmations (checks run on a real machine, every line read, will answer review feedback personally, nothing proprietary pasted into an AI tool) are left for the author.
- **Verification**: paste only output you actually produced. Never predict or infer check results or manual test outcomes.

## Running locally (Linux)

- `just dev-link-system` overlays this checkout's build onto an existing package install (binaries, PAM modules, polkit policy, extensions, a `pam_gaze.so` line in `/etc/pam.d/sudo`, a systemd drop-in, and TPM encryption unless `GAZE_DEV_TPM=0`). `just dev-unlink-system` restores; `just dev-link-status` reports.
- Or run the daemon in the foreground: `sudo systemctl stop gazed && sudo RUST_LOG=debug ./target/release/gazed`. Clients use whichever `gazed` owns the bus name.
- `just kde-harness [service] [rounds] [confdir=self]` drives a PAM service like KScreenLocker's noninteractive greeter and fails on any prompt.
- When testing PAM, keep a root shell open and use a non-critical service before `sudo` or login.
