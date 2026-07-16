# Development

This page covers source builds, tests, packaging, and Flatpak workflows for contributors.

For pull request workflow, testing expectations, and safety notes, see [Contributing](/guide/contributing).

## Prerequisites

Gaze targets Linux platform APIs (V4L2, PAM, TPM2/tss2, polkit, GTK4/libadwaita, Flatpak,
SELinux) that do not exist on macOS or Windows, so none of this builds natively there. If
you're not on Linux, skip straight to [Building without a Linux host (Docker)](#building-without-a-linux-host-docker).

- Rust 1.85+ (or install current stable via `rustup`)
- `just` 1.51+ (https://github.com/casey/just) for task automation
- `nfpm` (https://nfpm.goreleaser.com) for packaging
- `flatpak` and `flatpak-builder` (https://github.com/flatpak/flatpak-builder) for the Flatpak build, plus the Flathub remote and GNOME/Rust/LLVM SDKs. See [Flatpak build](#flatpak-build) below.

::: code-group

```bash [Debian/Ubuntu]
sudo apt install build-essential pkg-config clang libclang-dev \
  libopencv-dev libv4l-dev libpam0g-dev \
  libgtk-4-dev libadwaita-1-dev \
  libcairo2-dev libglib2.0-dev libgdk-pixbuf-2.0-dev \
  libpango1.0-dev libgraphene-1.0-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  flatpak flatpak-builder elfutils
```

```bash [Fedora/RHEL]
sudo dnf install @development-tools pkg-config clang clang-devel \
  opencv-devel libv4l-devel pam-devel \
  gtk4-devel libadwaita-devel \
  gstreamer1-devel gstreamer1-plugins-base-devel \
  checkpolicy policycoreutils \
  flatpak flatpak-builder elfutils
```

```bash [Arch Linux / Manjaro]
sudo pacman -S base-devel pkgconf clang llvm \
  opencv v4l-utils pam \
  gtk4 libadwaita \
  gstreamer gst-plugins-base \
  flatpak flatpak-builder elfutils
```

:::

Both OpenCV 4 and 5 work. On distros that ship OpenCV 5 (such as Arch Linux),
the `just` recipes automatically point the `opencv` crate at the `opencv5`
pkg-config name; when running `cargo` directly, set
`OPENCV_PKGCONFIG_NAME=opencv5` yourself.

## Setup

```bash
git clone https://github.com/gundulabs/gaze
cd gaze
just setup-hooks
just --list
```

Git hooks are local to each clone. `just setup-hooks` points Git at the tracked hook scripts so pre-commit checks stay up to date when the repo changes. CI still runs the same required checks for pushes and pull requests.

## Workspace layout

- `gaze`: the `gazed` daemon, ML pipeline, and user database.
- `gaze-cli`: the `gaze` CLI binary. It lives in its own crate so the client binary does not statically link ONNX Runtime (see warning below).
- `gaze-core`: shared camera/config/DBus library. Face detection sits behind the `detection` cargo feature (on by default); client crates opt out with `default-features = false`.
- `pam-gaze`, `pam-gaze-grosshack`: `cdylib` PAM modules; shared FFI/auth logic lives in `pam-gaze-core`.
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

```bash
just build-rust
just dev-link-system    # symlinks /usr/bin/gazed, /usr/bin/gaze, PAM modules, and extension over your build
```

`just dev-unlink-system` restores the package-installed files from backup. `just dev-link-status` shows what is currently linked.

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

`pam-gaze` and `pam-gaze-grosshack` build as `cdylib`s. After `just build-rust` you'll have:

- `target/release/libpam_gaze.so`
- `target/release/libpam_gaze_grosshack.so`

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

## Packaging

```bash
just package <deb | rpm | archlinux>
```

Package output:

- `dist/packages/`

## Flatpak build

The `flatpak`/`flatpak-builder` packages above are not enough on their own. The manifest
(`packaging/flatpak/com.gundulabs.Gaze.yml`) also needs the Flathub remote and the exact
GNOME runtime/SDK plus the Rust and LLVM SDK extensions it builds against. These versions
must match `.github/workflows/cd.yml` and `packaging/docker/entrypoint.sh`, so bump all
three together.

```bash
flatpak remote-add --user --if-not-exists flathub https://flathub.org/repo/flathub.flatpakrepo
flatpak install --user -y flathub \
  org.gnome.Sdk//49 \
  org.gnome.Platform//49 \
  org.freedesktop.Sdk.Extension.rust-stable//25.08 \
  org.freedesktop.Sdk.Extension.llvm20//25.08
```

Then build:

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
- For `build-flatpak`, the entrypoint lazily adds the Flathub remote and installs the same
  GNOME/Rust/LLVM SDKs listed above into a volume the first time a `flatpak` target runs.
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
