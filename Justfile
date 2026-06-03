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
[group("build")]
build-rust:
    cargo build --workspace --release

# Compile the SELinux policy module
[group("build")]
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

[arg("format", pattern="deb|rpm|archlinux")]
[env("MULTIARCH", multiarch)]
[env("PACKAGE_RELEASE", package_release)]
[env("VERSION", version)]
[private]
_nfpm config format:
    ARCH="{{ if format == "deb" { deb_arch } else { arch } }}" \
        {{ quote(nfpm) }} pkg -f {{ quote(config) }} --packager {{ format }} --target dist/packages

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
