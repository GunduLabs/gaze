# Development

This page covers source builds, tests, packaging, and Flatpak workflows for contributors.

For pull request workflow, testing expectations, and safety notes, see [Contributing](/guide/contributing).

## Prerequisites

- Rust 1.85+ (or install current stable via `rustup`)
- `just` 1.51+ (https://github.com/casey/just) for task automation
- `nfpm` (https://nfpm.goreleaser.com) for packaging
- `flatpak-builder` (https://github.com/flatpak/flatpak-builder) for flatpak

::: code-group

```bash [Debian/Ubuntu]
sudo apt install build-essential pkg-config clang libclang-dev \
  libopencv-dev libv4l-dev libpam0g-dev \
  libgtk-4-dev libadwaita-1-dev \
  libcairo2-dev libglib2.0-dev libgdk-pixbuf-2.0-dev \
  libpango1.0-dev libgraphene-1.0-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev
```

```bash [Fedora/RHEL]
sudo dnf install @development-tools pkg-config clang clang-devel \
  opencv-devel libv4l-devel pam-devel \
  gtk4-devel libadwaita-devel \
  gstreamer1-devel gstreamer1-plugins-base-devel \
  checkpolicy policycoreutils
```

```bash [Arch Linux / Manjaro]
sudo pacman -S base-devel pkgconf clang llvm \
  opencv v4l-utils pam \
  gtk4 libadwaita \
  gstreamer gst-plugins-base
```

:::

## Setup

```bash
git clone https://github.com/gundulabs/gaze
cd gaze
just setup-hooks
just --list
```

Git hooks are local to each clone. `just setup-hooks` points Git at the tracked hook scripts so pre-commit checks stay up to date when the repo changes. CI still runs the same required checks for pushes and pull requests.

## Build and test rust components

```bash
just build-rust
just test
just lint
just fmt-check
```

## Run a locally-built daemon

The daemon takes no CLI arguments — paths are compiled in:

- Config: `/etc/gaze/config.toml`
- User templates: `/var/lib/gaze/users`
- Models: `/var/cache/gaze`

It also owns `com.gundulabs.Gaze` on the **system** DBus bus, which requires root. You cannot run a second daemon as your user.

Easiest iteration loop: stop the installed service, run your build in the foreground.

```bash
sudo systemctl stop gazed
cargo build --workspace --release
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

The CLI and GUI need no special setup — they talk to whichever `gazed` currently owns the bus name:

```bash
./target/release/gaze list-faces
./target/release/gaze auth --verbose
./target/release/gaze-gui
```

## Iterating on the PAM module

`pam-gaze` and `pam-gaze-grosshack` build as `cdylib`s. After `cargo build --release` you'll have:

- `target/release/libpam_gaze.so`
- `target/release/libpam_gaze_grosshack.so`

To exercise them through real PAM, copy into the system PAM library directory (path is distro-specific):

```bash
# Debian/Ubuntu
sudo cp target/release/libpam_gaze.so /lib/x86_64-linux-gnu/security/pam_gaze.so

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

```bash
just build-flatpak
```

Output bundle:

- `dist/packages/com.gundulabs.Gaze.flatpak`

## Cleaning build artifacts

```bash
just clean
```
