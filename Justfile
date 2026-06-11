# Justfile for Gaze - https://gaze.gundulabs.com
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
    cargo build -p gaze --release
    cargo build -p gaze-cli -p gaze-gui -p pam-gaze -p pam-gaze-grosshack --release

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

# Build flatpak repo and bundle
[group("build")]
build-flatpak: prepare-flatpak-vendor prepare-flatpak-ort
    mkdir -p dist/packages dist/flatpak-repo

    flatpak-builder \
        --force-clean \
        --disable-rofiles-fuse \
        --jobs="${FLATPAK_BUILDER_JOBS:-2}" \
        --repo=dist/flatpak-repo \
        --arch="$(flatpak --default-arch)" \
        --default-branch=stable \
        --user \
        $( [ -n "${FLATPAK_GPG_SIGN:-}" ] && printf '%s' "--gpg-sign=${FLATPAK_GPG_SIGN}" ) \
        flatpak-build \
        packaging/flatpak/com.gundulabs.Gaze.yml

    arch="$(flatpak --default-arch)"; \
    flatpak build-bundle \
        --arch="$arch" \
        $( [ -n "${FLATPAK_GPG_SIGN:-}" ] && printf '%s' "--gpg-sign=${FLATPAK_GPG_SIGN}" ) \
        dist/flatpak-repo \
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
    tmp_config=$(mktemp)
    envsubst '$MULTIARCH' < {{ quote(config) }} > "$tmp_config"
    {{ quote(nfpm) }} pkg -f "$tmp_config" --packager {{ format }} --target dist/packages
    rm -f "$tmp_config"

[private]
_dist-packages:
    mkdir -p dist/packages

# Build nfpm packages for a given packager
[arg("format", pattern="deb|rpm|archlinux")]
[group("package")]
[parallel]
package format: build-rust build-selinux && (package-prebuilt format)

# Package already-built artifacts for a given packager
[arg("format", pattern="deb|rpm|archlinux")]
[group("package")]
package-prebuilt format: _dist-packages (_nfpm "packaging/nfpm.yaml" format) (_nfpm "packaging/nfpm-gui.yaml" format) (_nfpm "packaging/nfpm-gnome-extension.yaml" format) (_nfpm "packaging/nfpm-hyprlock.yaml" format)
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
    cargo test --workspace --release

# Check dependencies for known security advisories
[group("checks")]
audit:
    cargo audit

# Run clippy lints across the workspace
[group("checks")]
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Check formatting (does not write)
[group("checks")]
fmt-check:
    cargo fmt --all -- --check

# Apply formatting
[group("dev")]
fmt:
    cargo fmt --all

# Link the installed system runtime to this checkout's release build
[group("dev")]
dev-link-system: build-rust
    sudo scripts/dev-link-system.sh enable

# Restore package-installed files that dev-link-system replaced
[group("dev")]
dev-unlink-system:
    sudo scripts/dev-link-system.sh disable

# Show which installed Gaze paths are linked to this checkout
[group("dev")]
dev-link-status:
    scripts/dev-link-system.sh status
