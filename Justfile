# Justfile for Gaze: https://gaze.gundulabs.com
# Run `just` to see available targets.

set lazy

# Host architecture from just; can be overridden: just arch=aarch64 package deb
arch := env("ARCH", arch())
# Package version; defaults to the git tag (strip leading v)
version := if env("VERSION", "") != "" { env("VERSION") } else { trim_start_match(shell("git describe --tags --always"), "v") }
# Package release/revision; distro builds set this to keep artifacts distinct.
package_release := env("PACKAGE_RELEASE", "1")
# Required packaging tool; evaluated only by packaging recipes.
nfpm := require("nfpm")

# The opencv crate's build script only probes the `opencv4`/`opencv`
# pkg-config names, so on distros that ship OpenCV 5 (e.g. Arch) point it at
# `opencv5`. Empty (no override) when opencv4/opencv resolve or opencv5 doesn't.
opencv_env := shell("pkg-config --exists opencv4 2>/dev/null || pkg-config --exists opencv 2>/dev/null || ! pkg-config --exists opencv5 2>/dev/null || echo OPENCV_PKGCONFIG_NAME=opencv5")

# Derived vars
multiarch := if arch == "aarch64" { "aarch64-linux-gnu" } else { "x86_64-linux-gnu" }
deb_arch := if arch == "x86_64" { "amd64" } else if arch == "aarch64" { "arm64" } else { arch }

# List recipes when `just` is run without arguments.
[default]
[private]
default:
    @{{ quote(just_executable()) }} --justfile {{ quote(justfile()) }} --list

# ── build ─────────────────────────────────────────────────────────────────────

# Build all Rust workspace binaries (release)
#
# Split into two invocations so the daemon's `detection` feature on
# gaze-core does not unify into the client binaries. ONNX Runtime's
# constructors require AVX2 and crash on older CPUs (issue #14), so the
# CLI, GUI, and PAM modules must build without it.
[group("build")]
build-rust:
    {{ opencv_env }} cargo build -p gaze --release
    {{ opencv_env }} cargo build -p gaze-cli -p gaze-gui -p pam-gaze -p pam-gaze-grosshack --release

# Compile the SELinux policy module
[group("build")]
build-selinux:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p dist/selinux
    if command -v checkmodule >/dev/null 2>&1 && command -v semodule_package >/dev/null 2>&1; then
        checkmodule -M -m -o dist/selinux/gaze-gdm-camera.mod packaging/selinux/gaze-gdm-camera.te
        semodule_package -o dist/selinux/gaze-gdm-camera.pp -m dist/selinux/gaze-gdm-camera.mod
        rm -f dist/selinux/gaze-gdm-camera.mod
        echo "Built dist/selinux/gaze-gdm-camera.pp"
    else
        echo "WARNING: SELinux tools not found. Skipping SELinux policy build." >&2
    fi

[private]
prepare-flatpak-vendor:
    mkdir -p .flatpak-cache/cargo
    cargo vendor --locked --versioned-dirs > .flatpak-cache/cargo/config.toml

[private]
prepare-flatpak-ort:
    mkdir -p .flatpak-cache/ort
    arch="$(flatpak --default-arch)"; \
    case "$arch" in \
        x86_64) ort_arch="x64" ;; \
        aarch64) ort_arch="aarch64" ;; \
        *) echo "Unsupported Flatpak arch for ORT bootstrap: $arch" >&2; exit 1 ;; \
    esac; \
    ort_version="1.22.0"; \
    ort_file="onnxruntime-linux-${ort_arch}-${ort_version}.tgz"; \
    ort_url="https://github.com/microsoft/onnxruntime/releases/download/v${ort_version}/${ort_file}"; \
    if [ ! -s .flatpak-cache/ort/onnxruntime.tgz ]; then \
        curl -fsSL "$ort_url" -o .flatpak-cache/ort/onnxruntime.tgz; \
    fi

# flatpak-builder's state/build/repo dirs (ostree, needs xattrs + same-filesystem
# co-location). Default to the repo tree; the `docker` wrapper redirects them to an
# in-VM volume because the sshfs-backed bind mount can't host ostree.
flatpak_state_dir := env("FLATPAK_STATE_DIR", ".flatpak-builder")
flatpak_build_dir := env("FLATPAK_BUILD_DIR", "flatpak-build")
flatpak_repo_dir := env("FLATPAK_REPO_DIR", "dist/flatpak-repo")

# Build flatpak repo and bundle
[group("build")]
build-flatpak: prepare-flatpak-vendor prepare-flatpak-ort
    mkdir -p dist/packages {{ quote(flatpak_repo_dir) }}

    flatpak-builder \
        --force-clean \
        --disable-rofiles-fuse \
        --state-dir={{ quote(flatpak_state_dir) }} \
        --jobs="${FLATPAK_BUILDER_JOBS:-2}" \
        --repo={{ quote(flatpak_repo_dir) }} \
        --arch="$(flatpak --default-arch)" \
        --default-branch=stable \
        --user \
        $( [ -n "${FLATPAK_GPG_SIGN:-}" ] && printf '%s' "--gpg-sign=${FLATPAK_GPG_SIGN}" ) \
        {{ quote(flatpak_build_dir) }} \
        packaging/flatpak/com.gundulabs.Gaze.yml

    arch="$(flatpak --default-arch)"; \
    flatpak build-bundle \
        --arch="$arch" \
        $( [ -n "${FLATPAK_GPG_SIGN:-}" ] && printf '%s' "--gpg-sign=${FLATPAK_GPG_SIGN}" ) \
        {{ quote(flatpak_repo_dir) }} \
        "dist/packages/com.gundulabs.Gaze-${arch}.flatpak" \
        com.gundulabs.Gaze \
        stable

    install -Dm644 packaging/flatpak/com.gundulabs.Gaze.flatpakref dist/packages/com.gundulabs.Gaze.flatpakref
    install -Dm644 packaging/flatpak/com.gundulabs.Gaze.flatpakrepo dist/packages/com.gundulabs.Gaze.flatpakrepo
    if [ -n "${FLATPAK_GPG_SIGN:-}" ]; then \
        pubkey="$(gpg --export "${FLATPAK_GPG_SIGN}" | base64 -w0)"; \
        printf 'GPGKey=%s\n' "$pubkey" >> dist/packages/com.gundulabs.Gaze.flatpakref; \
        printf 'GPGKey=%s\n' "$pubkey" >> dist/packages/com.gundulabs.Gaze.flatpakrepo; \
    fi

# ── package ───────────────────────────────────────────────────────────────────

[arg("format", pattern="deb|rpm|archlinux")]
[env("MULTIARCH", multiarch)]
[env("PACKAGE_RELEASE", package_release)]
[env("VERSION", version)]
[private]
_nfpm config format:
    #!/usr/bin/env bash
    set -euo pipefail
    export ARCH="{{ if format == "deb" { deb_arch } else { arch } }}"
    # Arch bumps the OpenCV soname on every minor release, so pin the package
    # dependency to the soversion gazed actually linked; otherwise an opencv
    # upgrade leaves the daemon crash-looping on a missing library instead of
    # failing the pacman transaction.
    if [ "{{ format }}" = "archlinux" ]; then
        sover=$(objdump -p target/release/gazed | awk '/NEEDED/ && /libopencv_core\.so\./ { sub(/.*\.so\./, "", $2); print $2 }')
        [[ "$sover" =~ ^[0-9]+$ ]] || { echo "_nfpm: cannot read libopencv_core soversion from target/release/gazed" >&2; exit 1; }
        export OPENCV_MIN="$((sover / 100)).$((sover % 100))"
        export OPENCV_NEXT="$((sover / 100)).$((sover % 100 + 1))"
    fi
    tmp_config=$(mktemp)
    envsubst '$MULTIARCH $OPENCV_MIN $OPENCV_NEXT' < {{ quote(config) }} > "$tmp_config"
    {{ quote(nfpm) }} pkg -f "$tmp_config" --packager {{ format }} --target dist/packages
    rm -f "$tmp_config"

[private]
_dist-packages:
    mkdir -p dist/packages

# Assert the arch package embeds a post_upgrade() scriptlet (no-op for deb/rpm)
# and a version-bounded opencv dependency. Guards the nfpm archlinux
# postupgrade mapping (without it, upgrades skip the daemon-reload /
# polkit-restart / PAM setup in postinst-arch.sh) and the opencv soversion pin
# (without it, an Arch opencv bump crash-loops gazed instead of failing the
# pacman transaction).
[arg("format", pattern="deb|rpm|archlinux")]
[private]
_verify-arch format:
    #!/usr/bin/env bash
    set -euo pipefail
    [ "{{ format }}" = "archlinux" ] || exit 0
    pkg=$(ls -t dist/packages/gaze-[0-9]*.pkg.tar.* 2>/dev/null | head -n1 || true)
    [ -n "$pkg" ] || { echo "verify: no arch gaze package in dist/packages" >&2; exit 1; }
    if tar -xOf "$pkg" .INSTALL 2>/dev/null | grep -q 'post_upgrade *()'; then
        echo "verify: $(basename "$pkg") embeds post_upgrade() ✔"
    else
        echo "verify: FAIL: $(basename "$pkg") is missing post_upgrade(); arch upgrades will skip postinst-arch.sh" >&2
        exit 1
    fi
    pkginfo=$(tar -xOf "$pkg" .PKGINFO 2>/dev/null)
    if grep -Eq 'depend = opencv>=[0-9]+\.[0-9]+$' <<< "$pkginfo" \
        && grep -Eq 'depend = opencv<[0-9]+\.[0-9]+$' <<< "$pkginfo"; then
        echo "verify: $(basename "$pkg") pins opencv ($(grep -oE 'opencv[<>=]+[0-9.]+' <<< "$pkginfo" | tr '\n' ' ')) ✔"
    else
        echo "verify: FAIL: $(basename "$pkg") lacks a version-bounded opencv dependency; an opencv soname bump will crash-loop gazed" >&2
        exit 1
    fi

# Build nfpm packages for a given packager
[arg("format", pattern="deb|rpm|archlinux")]
[group("package")]
[parallel]
package format: build-rust build-selinux && (package-prebuilt format)

# Package already-built artifacts for a given packager
[arg("format", pattern="deb|rpm|archlinux")]
[group("package")]
package-prebuilt format: _dist-packages (_nfpm "packaging/nfpm.yaml" format) (_nfpm "packaging/nfpm-gui.yaml" format) (_nfpm "packaging/nfpm-gnome-extension.yaml" format) (_nfpm "packaging/nfpm-hyprlock.yaml" format) && (_verify-arch format)
    @echo "Packages written to dist/packages/"

# Remove all generated artifacts
[group("dev")]
clean:
    cargo clean
    rm -rf dist
    rm -rf flatpak-build .flatpak-builder
    rm -rf .flatpak-cache
    rm -rf vendor

# ── dev helpers ───────────────────────────────────────────────────────────────

# Enable Git hooks for this clone
[group("dev")]
setup-hooks:
    scripts/setup-hooks.sh

# Run the full test suite
[group("checks")]
test:
    {{ opencv_env }} cargo test --workspace --release

# Check dependencies for known security advisories
[group("checks")]
audit:
    cargo audit

# Run clippy lints across the workspace
[group("checks")]
lint:
    {{ opencv_env }} cargo clippy --workspace --all-targets -- -D warnings

# Check formatting (does not write)
[group("checks")]
fmt-check:
    cargo fmt --all -- --check

# Apply formatting
[group("dev")]
fmt:
    cargo fmt --all

# Link the installed system runtime to this checkout's release build
# (also enables TPM template encryption when a TPM is present; GAZE_DEV_TPM=0 skips it)
[group("dev")]
dev-link-system: build-rust
    sudo GAZE_DEV_TPM="${GAZE_DEV_TPM:-1}" scripts/dev-link-system.sh enable

# Restore package-installed files that dev-link-system replaced
[group("dev")]
dev-unlink-system:
    sudo scripts/dev-link-system.sh disable

# Show which installed Gaze paths are linked to this checkout
[group("dev")]
dev-link-status:
    scripts/dev-link-system.sh status

# ── docker (build the Linux targets on a non-Linux host) ────────────────────────

# Tag for the local Linux build-environment image
docker_image := env("GAZE_DOCKER_IMAGE", "gaze-build:local")

# Build (or refresh) the Linux build-environment image; cached after the first run
[group("docker")]
docker-image:
    docker build -t {{ quote(docker_image) }} -f packaging/docker/Dockerfile.build packaging/docker

# Run any build/package target inside the Linux container, e.g. `just docker build-rust`,
# `just docker build-flatpak`, `just docker package-prebuilt deb`. Artifacts land in dist/.
#
# flatpak-builder writes ostree (needs xattrs + same-filesystem co-location) which a
# sshfs-backed /work bind mount can't host, so its state/build/repo are redirected to a
# single in-VM volume (/flatpak); the final .flatpak bundle is a plain file and still
# lands in dist/packages.
[group("docker")]
docker target *args: docker-image
    docker run --rm --privileged \
        -v {{ quote(justfile_directory() + ":/work") }} \
        -v gaze-cargo-registry:/root/.cargo/registry \
        -v gaze-cargo-git:/root/.cargo/git \
        -v gaze-target:/work/target \
        -v gaze-flatpak:/root/.local/share/flatpak \
        -v gaze-flatpak-work:/flatpak \
        -e FLATPAK_STATE_DIR=/flatpak/state \
        -e FLATPAK_BUILD_DIR=/flatpak/build \
        -e FLATPAK_REPO_DIR=/flatpak/repo \
        -e CARGO_BUILD_JOBS -e FLATPAK_BUILDER_JOBS -e FLATPAK_GPG_SIGN \
        -e VERSION -e PACKAGE_RELEASE \
        -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
        {{ quote(docker_image) }} \
        {{ target }} {{ args }}
