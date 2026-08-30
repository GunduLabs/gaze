<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Development

This page covers source builds, tests, packaging, and Flatpak workflows for contributors.

For pull request workflow, testing expectations, and safety notes, see [Contributing](/guide/contributing).

## Prerequisites

Gaze targets Linux platform APIs (V4L2, PAM, TPM2/tss2, polkit, GTK4/libadwaita, Flatpak,
SELinux) that do not exist on macOS or Windows, so none of this builds natively there.

There are three ways to get an environment, and they all end at the same `just` recipes. Take
whichever asks the least of you.

### Nix, the shortest path

```bash
nix develop
```

That is the entire setup. The shell has the Rust toolchain and every native dependency (OpenCV,
GStreamer, GTK4, ONNX Runtime, tpm2-tss) already wired up. See the [Nix & NixOS guide](/guide/nixos).

### Docker, if you are not on Linux

```bash
just docker build-rust
```

Any recipe also runs inside a container that mirrors CI, so the host needs nothing but Docker.
See [Building without a Linux host](#building-without-a-linux-host-docker).

### Distro packages

Install the tooling:

- Rust 1.85+, via [rustup](https://rustup.rs)
- [`just`](https://github.com/casey/just) 1.51+, the task runner everything below goes through
- [`nfpm`](https://nfpm.goreleaser.com), only for `just package`
- [`flatpak-builder`](https://github.com/flatpak/flatpak-builder), only for `just build-flatpak`

CI pins its own versions in `.github/workflows/ci.yml` if you need to match them exactly. Then
the system libraries:

::: code-group

```bash [Debian/Ubuntu]
sudo apt install build-essential pkg-config clang libclang-dev \
  libopencv-dev libv4l-dev libpam0g-dev libtss2-dev libssl-dev \
  libgtk-4-dev libadwaita-1-dev \
  libcairo2-dev libglib2.0-dev libgdk-pixbuf-2.0-dev \
  libpango1.0-dev libgraphene-1.0-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good \
  gettext-base \
  flatpak flatpak-builder elfutils
```

```bash [Fedora/RHEL]
sudo dnf install @development-tools pkg-config clang clang-devel \
  opencv-devel libv4l-devel pam-devel tpm2-tss-devel openssl-devel \
  gtk4-devel libadwaita-devel \
  gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good \
  checkpolicy policycoreutils \
  gettext \
  flatpak flatpak-builder elfutils
```

```bash [openSUSE Tumbleweed]
sudo zypper install --no-recommends \
  clang clang-devel opencv-devel libv4l-devel pam-devel tpm2-0-tss-devel \
  libopenssl-devel gtk4-devel libadwaita-devel \
  gstreamer-devel gstreamer-plugins-base-devel gstreamer-plugins-good \
  checkpolicy policycoreutils pkgconf-pkg-config envsubst gcc gcc-c++ \
  flatpak flatpak-builder elfutils
```

```bash [Arch Linux / Manjaro]
sudo pacman -S base-devel pkgconf clang llvm \
  opencv v4l-utils pam tpm2-tss openssl \
  gtk4 libadwaita \
  gstreamer gst-plugins-base gst-plugins-good \
  gettext \
  flatpak flatpak-builder elfutils
```

:::

`libtss2-dev`/`tpm2-tss-devel`/`tpm2-tss`/`tpm2-0-tss-devel` and `libssl-dev`/`openssl-devel`/`openssl`/`libopenssl-devel` back the
`tss-esapi` and `openssl-sys` crates (the daemon seals the face-template key to the TPM);
`gettext-base`/`gettext`/`envsubst` provides `envsubst`, which the `package` recipe below needs.

Both OpenCV 4 and 5 work. On distros that ship OpenCV 5 (such as Arch Linux),
the `just` recipes automatically point the `opencv` crate at the `opencv5`
pkg-config name; when running `cargo` directly, set
`OPENCV_PKGCONFIG_NAME=opencv5` yourself.

Only `gaze-gui` needs gtk4 and libadwaita (`libgtk-4-dev`/`gtk4-devel`/`gtk4`,
`libadwaita-1-dev`/`libadwaita-devel`/`libadwaita`, and on Debian/Ubuntu the
cairo, glib, gdk-pixbuf, pango, and graphene headers listed with them). For a
TUI-only checkout, set `GAZE_GUI=0` (also `false`, `no`, or `off`) and skip
those packages: `build-rust`, `build-rust-openvino`, `test`, and `lint` then
leave `gaze-gui` out, and `dev-link-system` skips the binary it never built.
The daemon, the `gaze` TUI, the CLI, and the PAM modules are unaffected. Like
`OPENCV_PKGCONFIG_NAME`, this only covers those `just` recipes: a bare `cargo
build`/`cargo test` still builds every workspace member, and the packaging paths
(`package`, `build-flatpak`, and the spec `srpm` feeds) always include the GUI,
so building packages still needs the GUI dependencies installed.

## Setup

```bash
git clone https://github.com/gundulabs/gaze
cd gaze
just setup-hooks
just build-rust
just test
```

That is a full working checkout. `just --list` shows every other recipe.

Git hooks are local to each clone. `just setup-hooks` points Git at the tracked hook scripts so pre-commit checks stay up to date when the repo changes. CI still runs the same required checks for pushes and pull requests.

## Workspace layout

- `gaze`: the `gazed` daemon, ML pipeline, and user database.
- `gaze-cli`: the `gaze` CLI binary. It lives in its own crate so the client binary does not statically link ONNX Runtime (see warning below).
- `gaze-core`: shared camera/config/DBus library. Face detection sits behind the `detection` cargo feature (on by default); client crates opt out with `default-features = false`.
- `pam-gaze`: `cdylib` PAM module.
- `gaze-gui`: GTK4/libadwaita app. `gnome-shell-extension/` is packaged separately.

## Build and test rust components

```bash
just build-rust
just test
just lint
just fmt-check
just audit        # check dependencies for known CVEs
just fmt          # apply formatting (fmt-check only checks)
```

The default build supports CPU inference only. To build the daemon and
configuration tools with OpenVINO support, provide an OpenVINO-enabled system
ONNX Runtime and run:

```bash
ORT_STRATEGY=system \
ORT_LIB_LOCATION=/path/to/onnxruntime/lib \
ORT_PREFER_DYNAMIC_LINK=1 \
just build-rust-openvino
```

The `openvino` Cargo feature is explicit. The build fails when that feature is
enabled without a matching ONNX Runtime library.

::: warning Keep the `api-21` feature on the `ort` dependency
`gaze` and `gaze-core` depend on `ort` with `default-features = false` and
`api-21`, which pins the ONNX Runtime C API version the binaries ask for. `ort`
defaults to the newest API its release targets, and a runtime older than that
makes ONNX Runtime hand back a null API pointer, which `ort` turns into a panic
during process teardown and a core dump. Anything that links a system runtime
(Nix, Flatpak, RPM source builds, `ORT_STRATEGY=system` in CI) can be as old as
ONNX Runtime 1.21, so an `ort` upgrade must keep the `api-21` feature rather than
inherit the new default. `gazed` also checks the loaded runtime before touching
`ort`, and `gaze-core`'s `inference::` tests fail against a runtime that is too
old.

`api-21` is also the newest API level Gaze can ask for safely. From `api-22` on,
`ort`'s session builder sets an automatic execution-provider selection policy on
every session, which makes ONNX Runtime pick execution providers from the
platform's hardware device list instead of installing the built-in CPU provider.
ONNX Runtime 1.22 has no device discovery on Linux, so that list is empty and the
selection code dereferences it unchecked and aborts the process, even though Gaze
only ever asked for CPU inference. Gaze uses nothing that needs API 22 or newer,
so staying on `api-21` keeps session creation on the path that installs the CPU
provider directly.
:::

The OpenVINO-enabled binary supports Intel CPU, GPU, and NPU devices. The
`device` value in `/etc/gaze/config.toml` selects the device at run time; GPU
and NPU do not require separate builds. An installation with OpenVINO support
should set `execution_provider = "openvino"` and `device = "npu"` in its
installed configuration. If OpenVINO setup fails at run time, Gaze still falls
back to the ONNX Runtime CPU provider.

::: warning OpenVINO is a source build only
The released `.deb`, `.rpm`, Arch, and Flatpak packages are all produced by
`just build-rust`, so none of them include OpenVINO. Getting it means building
from source with `just build-rust-openvino` against your own OpenVINO-enabled
ONNX Runtime.
:::

CI does not cover the OpenVINO features either: `just lint`, `just test`, and
`just build-rust` all build CPU-only. Run `just test-openvino`,
`just lint-openvino`, and `just build-rust-openvino` by hand before changing
anything behind the `openvino` or `openvino-config` features. `just test` does
compile `gaze-core` with `openvino-config` alone, which is what the CLI and GUI
ship with, but that path needs no OpenVINO runtime.

::: warning Build with `just build-rust`, not `cargo build --workspace`
`just build-rust` builds the daemon and the clients in separate cargo invocations so feature unification cannot link ONNX Runtime into the CLI, GUI, or PAM modules. ONNX Runtime's startup code requires AVX2, and a single workspace build would silently reintroduce crashes on older CPUs.
:::

## Run a locally-built daemon

The daemon takes no CLI arguments; paths are compiled in:

- Config: `/etc/gaze/config.toml`
- User templates: `/var/lib/gaze/users`
- Models: `/var/cache/gaze`

It also owns `com.gundulabs.Gaze` on the **system** DBus bus, which requires root. You cannot run a second daemon as your user.

**Option A: link your build over the installed files** (easier for repeated iteration):

This overlays your checkout onto an *existing* package install; it does not install the
package itself. If you've never installed Gaze on this machine, build and install a package
once first (`just package rpm` and `sudo <package manager> install dist/packages/gaze-*.rpm`,
or the `deb`/`archlinux` equivalent); `dev-link-system` fails fast with a pointer back here if
`gazed.service` isn't installed yet.

```bash
just build-rust
just dev-link-system    # runs scripts/dev-link-system.sh under sudo itself
```

`dev-link-system` (`scripts/dev-link-system.sh enable`) does more than swap binaries:

- Links `/usr/bin/gazed`, `/usr/bin/gaze`, the PAM modules, the polkit policy, and the GNOME
  extension (system-wide and current-user) over the package-installed files. `/usr/bin/gaze-gui`
  is linked too when the build produced it, and skipped after a `GAZE_GUI=0` build.
- Adds a `pam_gaze.so` line to `/etc/pam.d/sudo` if one isn't already there.
- Installs a systemd drop-in for `gazed` that clears `InaccessiblePaths=/home /root` so the
  packaged unit can execute a binary linked from your checkout, then restarts `gazed`.
- If a TPM is present, turns on `[storage] encrypt_templates` and seals a key to it (set
  `GAZE_DEV_TPM=0` to skip this).

`just dev-unlink-system` reverses all of the above from backup, including the PAM line and
the encryption setting. `just dev-link-status` shows what is currently linked, the TPM/encryption
state, and how many templates on disk are encrypted.

**Option B: run the daemon in the foreground**:

```bash
sudo systemctl stop gazed
just build-rust
sudo RUST_LOG=debug ./target/release/gazed
```

`RUST_LOG` accepts standard `tracing` filters (`info`, `debug`, `gaze=trace`, etc.). Ctrl-C to stop, then `sudo systemctl start gazed` when you're done to restore the system daemon.

If you've never installed Gaze on this machine, you also need the DBus policy and a config file in place before the daemon can claim its name or load. The simplest way is to install the package once, then iterate on the binary:

```bash
sudo install -Dm644 packaging/config/com.gundulabs.Gaze.conf \
  /etc/dbus-1/system.d/com.gundulabs.Gaze.conf
sudo install -Dm644 packaging/config/config.toml /etc/gaze/config.toml
sudo systemctl reload dbus
```

The CLI and GUI need no special setup; they talk to whichever `gazed` currently owns the bus name:

```bash
./target/release/gaze list-faces
./target/release/gaze auth --verbose
./target/release/gaze-gui
```

## Iterating on the PAM module

`pam-gaze` builds as a `cdylib`. After `just build-rust` you'll have:

- `target/release/libpam_gaze.so`

To exercise them through real PAM, copy into the system PAM library directory (path is distro-specific):

```bash
# Debian/Ubuntu up to 25.10
sudo cp target/release/libpam_gaze.so /lib/x86_64-linux-gnu/security/pam_gaze.so

# Ubuntu 26.04+ (libpam looks in /usr/lib/security)
sudo cp target/release/libpam_gaze.so /usr/lib/security/pam_gaze.so

# Fedora/RHEL
sudo cp target/release/libpam_gaze.so /lib64/security/pam_gaze.so

# Arch
sudo cp target/release/libpam_gaze.so /usr/lib/security/pam_gaze.so
```

::: warning Don't lock yourself out
Before touching PAM files, **keep a second terminal open with an active root shell** (`sudo -s`). If the module crashes or misbehaves, you can revert from that shell. Test against a non-critical service first (e.g. add a line to `/etc/pam.d/su` or a custom service), not `system-auth` or `sudo`.
:::

Quickest end-to-end test once the `.so` is in place:

```bash
sudo -k   # invalidate cached sudo credentials
sudo -v   # force a fresh PAM prompt
```

## Iterating on the GNOME extension

The extension source lives in `gnome-shell-extension/`. To run it from the tree without packaging:

```bash
mkdir -p ~/.local/share/gnome-shell/extensions
ln -sfn "$PWD/gnome-shell-extension" \
  ~/.local/share/gnome-shell/extensions/gaze@gundulabs.com

# compile the gsettings schema once
glib-compile-schemas ~/.local/share/gnome-shell/extensions/gaze@gundulabs.com/schemas

# on Xorg: Alt+F2 then `r`. On Wayland: log out and back in.
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

Watch shell logs while you iterate:

```bash
journalctl -f /usr/bin/gnome-shell
```

For the unlock-dialog session mode (lock screen), changes only take effect after a fresh lock, not a shell reload.

## Checking the KDE lock screen without Plasma

`just kde-harness` drives a PAM service exactly the way KScreenLocker's greeter
drives its noninteractive biometric slot: it renders error messages, discards
info messages, and fails loudly if the module issues a prompt (which would hang
the real greeter for the rest of the lock). Pass a service and a round count to
emulate re-arming after a wrong password:

```bash
just kde-harness kde-fingerprint 2
```

## Packaging

```bash
just package <deb | rpm | archlinux>
```

Package output:

- `dist/packages/`

## Flatpak build

The build recipe adds the Flathub remote and installs or updates the GNOME runtime/SDK
and Rust/LLVM extensions declared by the manifest, so their versions have a single source
of truth. Build with:

```bash
just build-flatpak
```

This runs two `[private]` prep recipes first (`prepare-flatpak-vendor`, `prepare-flatpak-ort`),
so the first run needs network access even though the sandboxed build itself is `--offline`:

- `cargo vendor --locked --versioned-dirs` populates `.flatpak-cache/cargo` from crates.io.
- It downloads the pinned ONNX Runtime release tarball into `.flatpak-cache/ort`.

Both are cached under `.flatpak-cache/` (removed by `just clean`), so only the first build
per checkout pays the network/OpenCV-from-source cost; expect that first build to take a
while, since OpenCV compiles from source inside the sandbox.

Output bundle:

- `dist/packages/com.gundulabs.Gaze-<arch>.flatpak` (e.g. `com.gundulabs.Gaze-x86_64.flatpak`)
- `dist/packages/com.gundulabs.Gaze.flatpakref` and `.flatpakrepo` (only meaningful once published to a real repo; fine to ignore for local builds)

Set `FLATPAK_GPG_SIGN=<key-id>` to sign the repo/bundle; leave it unset for local builds.

## Building without a Linux host (Docker)

macOS and Windows can't run any of the recipes above natively. `just docker <target>` runs
the same `just` target inside a disposable Ubuntu container that mirrors the CI toolchain
(`packaging/docker/Dockerfile.build`), so this works for `build-rust`, `build-flatpak`,
`package <deb|rpm|archlinux>`, and so on, with no local Rust/OpenCV/Flatpak setup needed on
the host:

```bash
just docker build-rust
just docker build-flatpak
just docker package deb
```

Requirements on the host:

- Docker (or a Docker-compatible runtime; Colima works on macOS).
- The container runs `--privileged`, which `flatpak-builder`'s ostree backend needs.

Notes:

- `just docker-image` builds (and caches) the build image; `just docker <target>` builds it
  automatically on first use.
- Cargo registry, build target, and Flatpak state persist in named Docker volumes across
  runs, so repeat builds don't re-download crates or the GNOME SDK.
- For `build-flatpak`, the recipe installs the manifest's GNOME/Rust/LLVM dependencies into
  a volume the first time the target runs.
- If your checkout lives on a sshfs-backed mount (e.g. a Colima VM on an external/network
  drive), Flatpak's ostree repo can't live on that mount. The wrapper already redirects
  `flatpak-builder`'s state/build/repo dirs to an in-VM Docker volume, so this works out of
  the box. Don't override `FLATPAK_STATE_DIR`/`FLATPAK_BUILD_DIR`/`FLATPAK_REPO_DIR` back
  onto the bind mount.
- Artifacts land in `dist/` on the host same as a native build, re-owned to your host
  user/group when the container runs as root (native Docker); on uid-mapping backends like
  Colima's sshfs, files already belong to you and are left alone.

## Cleaning build artifacts

```bash
just clean
```
