# Justfile for Gaze — https://gaze.gundulabs.com
# Run `just` to see available targets.

# Host architecture from just; can be overridden: just arch=aarch64 package deb
arch := env("ARCH", arch())
# Package version; defaults to the git tag (strip leading v)
version := env("VERSION", `git describe --tags --always | sed 's/^v//'`)

# Derived vars
multiarch := if arch == "aarch64" { "aarch64-linux-gnu" } else { "x86_64-linux-gnu" }
deb_arch := if arch == "x86_64" { "amd64" } else if arch == "aarch64" { "arm64" } else { arch }

export ARCH      := arch
export VERSION   := version
export MULTIARCH := multiarch

# ── defaults ─────────────────────────────────────────────────────────────────

[private]
default:
    @just --list

# ── build ─────────────────────────────────────────────────────────────────────

# Build all Rust workspace binaries (release)
build-rust:
    cargo build --workspace --release

# Compile the SELinux policy module
build-selinux:
    mkdir -p dist/selinux
    checkmodule -M -m -o dist/selinux/gaze-gdm-camera.mod packaging/selinux/gaze-gdm-camera.te
    semodule_package -o dist/selinux/gaze-gdm-camera.pp -m dist/selinux/gaze-gdm-camera.mod
    rm -f dist/selinux/gaze-gdm-camera.mod
    @echo "Built dist/selinux/gaze-gdm-camera.pp"

[private]
prepare-flatpak-vendor:
    mkdir -p .cargo
    cargo vendor --locked --versioned-dirs > .cargo/config.toml

[private]
prepare-flatpak-ort:
    mkdir -p .flatpak-cache/ort
    arch="$(flatpak --default-arch)"; \
    case "$arch" in \
        x86_64) ort_arch="x64" ;; \
        aarch64) ort_arch="aarch64" ;; \
        *) echo "Unsupported Flatpak arch for ORT bootstrap: $arch" >&2; exit 1 ;; \
    esac; \
    ort_version="1.23.2"; \
    ort_file="onnxruntime-linux-${ort_arch}-${ort_version}.tgz"; \
    ort_url="https://github.com/microsoft/onnxruntime/releases/download/v${ort_version}/${ort_file}"; \
    if [ ! -s .flatpak-cache/ort/onnxruntime.tgz ]; then \
        curl -fsSL "$ort_url" -o .flatpak-cache/ort/onnxruntime.tgz; \
    fi

# Build flatpak repo and bundle
build-flatpak: prepare-flatpak-vendor prepare-flatpak-ort
    mkdir -p dist/packages dist/flatpak-repo

    flatpak-builder \
        --force-clean \
        --repo=dist/flatpak-repo \
        --arch="$(flatpak --default-arch)" \
        --user \
        $( [ -n "${FLATPAK_GPG_SIGN:-}" ] && printf '%s' "--gpg-sign=${FLATPAK_GPG_SIGN}" ) \
        flatpak-build \
        packaging/flatpak/com.gundulabs.Gaze.yml

    flatpak build-bundle \
        dist/flatpak-repo \
        dist/packages/com.gundulabs.Gaze.flatpak \
        com.gundulabs.Gaze

# ── package ───────────────────────────────────────────────────────────────────

[private]
_nfpm config format:
    ARCH="{{ if format == "deb" { deb_arch } else { arch } }}" MULTIARCH="{{ multiarch }}" VERSION="{{ version }}" \
        nfpm pkg -f {{ config }} --packager {{ format }} --target dist/packages

# Build nfpm packages for a given packager
package format: build-rust build-selinux
    mkdir -p dist/packages
    just _nfpm packaging/nfpm.yaml {{ format }}
    just _nfpm packaging/nfpm-gui.yaml {{ format }}
    just _nfpm packaging/nfpm-gnome-extension.yaml {{ format }}
    @echo "Packages written to dist/packages/"

# Remove all generated artifacts
clean:
    cargo clean
    rm -rf dist
    rm -rf flatpak-build .flatpak-builder
    rm -rf .flatpak-cache
    rm -rf vendor
    rm -f .cargo/config.toml

# ── dev helpers ───────────────────────────────────────────────────────────────

# Run the full test suite
test:
    cargo test --workspace --release

# Run clippy lints across the workspace
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Check formatting (does not write)
fmt-check:
    cargo fmt --all -- --check

# Apply formatting
fmt:
    cargo fmt --all
