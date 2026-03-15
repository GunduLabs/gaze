#!/bin/sh
# Gaze installer — https://gaze.gundulabs.com/install.sh
# Usage: curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
REPO="GunduLabs/gaze"
GITHUB="https://github.com/${REPO}"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

need() {
    command -v "$1" >/dev/null 2>&1 || { red "error: '$1' is required but not installed."; exit 1; }
}

need curl
need grep
need uname
need awk

bold "Gaze installer"
echo ""

# ── architecture ──────────────────────────────────────────────────────────────

ARCH="$(uname -m)"
case "$ARCH" in
    x86_64)  PKG_ARCH="x86_64" ;;
    aarch64) PKG_ARCH="aarch64" ;;
    *) red "Unsupported architecture: $ARCH"; exit 1 ;;
esac

# ── distro detection ──────────────────────────────────────────────────────────

if [ ! -f /etc/os-release ]; then
    red "Cannot detect Linux distribution (missing /etc/os-release)"
    exit 1
fi
# shellcheck disable=SC1091
. /etc/os-release
DISTRO_ID="${ID}"
DISTRO_LIKE="${ID_LIKE:-}"

is_rpm() {
    case "$DISTRO_ID $DISTRO_LIKE" in
        *fedora*|*rhel*|*centos*|*rocky*|*alma*) return 0 ;;
    esac
    return 1
}

is_deb() {
    case "$DISTRO_ID $DISTRO_LIKE" in
        *debian*|*ubuntu*) return 0 ;;
    esac
    return 1
}

is_arch() {
    case "$DISTRO_ID $DISTRO_LIKE" in
        *arch*|*manjaro*) return 0 ;;
    esac
    return 1
}

if ! is_rpm && ! is_deb && ! is_arch; then
    red "Unsupported distribution: $DISTRO_ID"
    echo "Supported: Fedora, RHEL/CentOS/AlmaLinux/Rocky, Debian, Ubuntu, Arch Linux, Manjaro"
    exit 1
fi

# ── latest version ───────────────────────────────────────────────────────────

echo "Fetching latest release..."
VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | grep -o '"v[^"]*"' | tr -d '"')"
if [ -z "$VERSION" ]; then
    red "Could not fetch latest release from GitHub."
    exit 1
fi
echo "Latest version: $VERSION"
V="${VERSION#v}"

# ── download + install standalone packages ───────────────────────────────────

BASE_URL="${GITHUB}/releases/download/${VERSION}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if is_deb; then
    DEB_ARCH="amd64"
    [ "$PKG_ARCH" = "aarch64" ] && DEB_ARCH="arm64"
    echo "Downloading standalone packages..."
    curl -fsSL --progress-bar -o "${TMP}/gaze.deb" "${BASE_URL}/gaze_${V}_${DEB_ARCH}.deb"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gui.deb" "${BASE_URL}/gaze-gui_${V}_${DEB_ARCH}.deb"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gnome-extension.deb" "${BASE_URL}/gaze-gnome-extension_${V}_${DEB_ARCH}.deb"
    apt-get install -y "${TMP}/gaze.deb" "${TMP}/gaze-gui.deb" "${TMP}/gaze-gnome-extension.deb"

elif is_rpm; then
    echo "Downloading standalone packages..."
    curl -fsSL --progress-bar -o "${TMP}/gaze.rpm" "${BASE_URL}/gaze-${V}-1.${PKG_ARCH}.rpm"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gui.rpm" "${BASE_URL}/gaze-gui-${V}-1.${PKG_ARCH}.rpm"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gnome-extension.rpm" "${BASE_URL}/gaze-gnome-extension-${V}-1.${PKG_ARCH}.rpm"

    if command -v dnf >/dev/null 2>&1; then
        dnf install -y "${TMP}/gaze.rpm" "${TMP}/gaze-gui.rpm" "${TMP}/gaze-gnome-extension.rpm"
    else
        rpm -Uvh "${TMP}/gaze.rpm" "${TMP}/gaze-gui.rpm" "${TMP}/gaze-gnome-extension.rpm"
    fi

    if command -v authselect >/dev/null 2>&1; then
        rm -rf /etc/authselect/custom/gaze 2>/dev/null || true
        # Fix stale authselect.conf pointing to bare "gaze" instead of "vendor/gaze"
        if [ -f /etc/authselect/authselect.conf ]; then
            current=$(head -1 /etc/authselect/authselect.conf)
            if [ "$current" = "gaze" ]; then
                echo "vendor/gaze" > /etc/authselect/authselect.conf
            fi
        fi
        authselect select vendor/gaze --force || true
    fi

elif is_arch; then
    echo "Downloading standalone packages..."
    curl -fsSL --progress-bar -o "${TMP}/gaze.pkg.tar.zst" "${BASE_URL}/gaze-${V}-1-${PKG_ARCH}.pkg.tar.zst"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gui.pkg.tar.zst" "${BASE_URL}/gaze-gui-${V}-1-${PKG_ARCH}.pkg.tar.zst"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gnome-extension.pkg.tar.zst" "${BASE_URL}/gaze-gnome-extension-${V}-1-${PKG_ARCH}.pkg.tar.zst"
    pacman -U --noconfirm "${TMP}/gaze.pkg.tar.zst" "${TMP}/gaze-gui.pkg.tar.zst" "${TMP}/gaze-gnome-extension.pkg.tar.zst"
fi

systemctl restart gazed 2>/dev/null || true

# ── done ─────────────────────────────────────────────────────────────────────

echo ""
green "Gaze installed successfully!"
echo ""
echo "  Enroll your face:    gaze add-face <name>"
echo "  Test authentication: gaze auth"
echo "  GUI:                 gaze-gui"
echo ""
echo "Docs: https://gaze.gundulabs.com"
