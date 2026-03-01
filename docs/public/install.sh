#!/bin/sh
set -e

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

bold "Gaze installer"
echo ""

# Detect architecture
ARCH="$(uname -m)"
case "$ARCH" in
    x86_64)  PKG_ARCH="x86_64" ;;
    aarch64) PKG_ARCH="aarch64" ;;
    *) red "Unsupported architecture: $ARCH"; exit 1 ;;
esac

# Detect distro
if [ -f /etc/os-release ]; then
    . /etc/os-release
    DISTRO_ID="${ID}"
    DISTRO_LIKE="${ID_LIKE:-}"
else
    red "Cannot detect Linux distribution (missing /etc/os-release)"
    exit 1
fi

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
    case "$DISTRO_ID" in
        arch|manjaro|endeavouros) return 0 ;;
    esac
    return 1
}

if is_rpm; then
    PKG_EXT="rpm"
    PKG_NAME="gaze-latest-1.${PKG_ARCH}.rpm"
elif is_deb; then
    PKG_EXT="deb"
    case "$PKG_ARCH" in
        x86_64)  DEB_ARCH="amd64" ;;
        aarch64) DEB_ARCH="arm64" ;;
    esac
    PKG_NAME="gaze_latest_${DEB_ARCH}.deb"
elif is_arch; then
    PKG_EXT="pkg.tar.zst"
    PKG_NAME="gaze-latest-1-${PKG_ARCH}.pkg.tar.zst"
else
    red "Unsupported distribution: $DISTRO_ID"
    echo "Supported: Fedora, RHEL, Debian, Ubuntu, Arch"
    exit 1
fi

# Get latest release version
echo "Fetching latest release..."
VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | grep -o '"v[^"]*"' | tr -d '"')"
if [ -z "$VERSION" ]; then
    red "Could not fetch latest release from GitHub."
    exit 1
fi
echo "Latest version: $VERSION"

# Build download URL
BASE_URL="${GITHUB}/releases/download/${VERSION}"
case "$PKG_EXT" in
    rpm)         URL="${BASE_URL}/gaze-${VERSION#v}-1.${PKG_ARCH}.rpm" ;;
    deb)         URL="${BASE_URL}/gaze_${VERSION#v}_${DEB_ARCH}.deb" ;;
    pkg.tar.zst) URL="${BASE_URL}/gaze-${VERSION#v}-1-${PKG_ARCH}.pkg.tar.zst" ;;
esac

# Download
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
PKG_FILE="${TMP}/gaze.${PKG_EXT}"

echo "Downloading $URL ..."
curl -fsSL --progress-bar -o "$PKG_FILE" "$URL"

# Install
echo "Installing..."
if is_rpm; then
    sudo rpm -Uvh --force "$PKG_FILE"
    sudo systemctl enable --now gazed
    sudo authselect select vendor/gaze --force 2>/dev/null || true
elif is_deb; then
    sudo dpkg -i "$PKG_FILE" || sudo apt-get install -f -y
elif is_arch; then
    sudo pacman -U --noconfirm "$PKG_FILE"
fi

echo ""
green "Gaze installed successfully!"
echo ""
echo "  Enroll your face:    gaze add-face myface"
echo "  Test authentication: gaze auth"
echo "  GUI:                 gaze-gui"
echo ""
echo "Docs: https://gaze.gundulabs.com"
