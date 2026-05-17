# Justfile for Gaze - https://gaze.gundulabs.com
# Run `just` to see available targets.

# Host architecture from just; can be overridden: just arch=aarch64 package deb
arch := env("ARCH", arch())
# Package version; defaults to the git tag (strip leading v)
version := env("VERSION", `git describe --tags --always | sed 's/^v//'`)
# Package release/revision; distro builds set this to keep artifacts distinct.
package_release := env("PACKAGE_RELEASE", "1")

# Derived vars
multiarch := if arch == "aarch64" { "aarch64-linux-gnu" } else { "x86_64-linux-gnu" }
deb_arch := if arch == "x86_64" { "amd64" } else if arch == "aarch64" { "arm64" } else { arch }

export ARCH      := arch
export VERSION   := version
export MULTIARCH := multiarch
export PACKAGE_RELEASE := package_release

# ── build ─────────────────────────────────────────────────────────────────────

# Build all Rust workspace binaries (release)
build-rust:
    cargo build --workspace --release

# Compile the SELinux policy module
build-selinux:
    mkdir -p dist/selinux
    if command -v checkmodule >/dev/null 2>&1 && command -v semodule_package >/dev/null 2>&1; then \
        checkmodule -M -m -o dist/selinux/gaze-gdm-camera.mod packaging/selinux/gaze-gdm-camera.te; \
        semodule_package -o dist/selinux/gaze-gdm-camera.pp -m dist/selinux/gaze-gdm-camera.mod; \
        rm -f dist/selinux/gaze-gdm-camera.mod; \
        echo "Built dist/selinux/gaze-gdm-camera.pp"; \
    else \
        echo "WARNING: SELinux tools not found. Skipping SELinux policy build." >&2; \
    fi

# ── package ───────────────────────────────────────────────────────────────────

[private]
_nfpm config format:
    ARCH="{{ if format == "deb" { deb_arch } else { arch } }}" MULTIARCH="{{ multiarch }}" VERSION="{{ version }}" \
        nfpm pkg -f {{ config }} --packager {{ format }} --target dist/packages

# Build nfpm packages for a given packager
package format: build-rust build-selinux
    just package-prebuilt {{ format }}

# Package already-built artifacts for a given packager
package-prebuilt format:
    mkdir -p dist/packages
    just _nfpm packaging/nfpm.yaml {{ format }}
    just _nfpm packaging/nfpm-gui.yaml {{ format }}
    just _nfpm packaging/nfpm-gnome-extension.yaml {{ format }}
    @echo "Packages written to dist/packages/"

# Remove all generated artifacts
clean:
    cargo clean
    rm -rf dist
    rm -rf vendor

# ── dev helpers ───────────────────────────────────────────────────────────────

# Enable Git hooks for this clone
setup-hooks:
    scripts/setup-hooks.sh

# Run the full test suite
test:
    cargo test --workspace --release

# Check dependencies for known security advisories
audit:
    cargo audit

# Run clippy lints across the workspace
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Check formatting (does not write)
fmt-check:
    cargo fmt --all -- --check

# Apply formatting
fmt:
    cargo fmt --all
