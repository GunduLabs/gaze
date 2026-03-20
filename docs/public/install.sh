#!/bin/sh
# Gaze installer - https://gaze.gundulabs.com/install.sh
# Usage: curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
REPO="GunduLabs/gaze"
GITHUB="https://github.com/${REPO}"
AUTO_YES=0
ATOMIC_FEDORA=0

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

while [ "$#" -gt 0 ]; do
    case "$1" in
        -y|--yes) AUTO_YES=1 ;;
        *) red "Unknown option: $1"; exit 1 ;;
    esac
    shift
done

need() {
    command -v "$1" >/dev/null 2>&1 || { red "error: '$1' is required but not installed."; exit 1; }
}

prompt_continue() {
    if [ "$AUTO_YES" -eq 1 ]; then
        return 0
    fi

    echo ""
    printf "Continue? [y/N]: "
    if [ -r /dev/tty ]; then
        read answer </dev/tty
    else
        red "No interactive terminal available. Re-run with --yes for non-interactive install."
        exit 1
    fi

    case "$answer" in
        y|Y|yes|YES) return 0 ;;
        *) echo "Aborted."; exit 0 ;;
    esac
}

need curl
need grep
need uname
need awk
need id

bold "Gaze installer"
echo ""

if [ "$(id -u)" -ne 0 ]; then
    red "Please run this installer as root, for example: curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh"
    exit 1
fi

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
VARIANT_ID="${VARIANT_ID:-}"

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

echo ""
bold "This installer will:"
echo "1. Detect your Linux distribution and architecture"
echo "2. Download Gaze ${VERSION} packages from the latest GitHub release"
echo "3. Install the packages needed for face authentication on your system"
echo "4. Configure your distro repository for future updates"
echo "5. Finish with the next steps for your distro"
echo ""

if is_deb; then
    echo "Detected platform: Debian/Ubuntu (${PKG_ARCH})"
    echo "Package manager: apt"
    echo ""
    bold "Planned steps for this system:"
    echo "- Download gaze, gaze-gui, and gaze-gnome-extension DEBs"
    echo "- Install them with apt"
    echo "- Configure the apt repository for future updates"
    echo "- Set up the PAM modules through pam-auth-update if available"
    echo "- Enable the Gaze daemon"
elif is_rpm; then
    echo "Detected platform: Fedora/RHEL family (${PKG_ARCH})"
    if command -v dnf >/dev/null 2>&1; then
        echo "Package manager: dnf"
    else
        echo "Package manager: rpm"
    fi
    echo ""
    bold "Planned steps for this system:"
    echo "- Download gaze, gaze-gui, and gaze-gnome-extension RPMs"
    echo "- Install them with your RPM package manager"
    echo "- Configure the dnf repository for future updates"
    echo "- Set up the PAM modules through authselect if available"
    echo "- Enable the Gaze daemon"
elif is_arch; then
    echo "Detected platform: Arch/Manjaro (${PKG_ARCH})"
    echo "Package manager: pacman"
    echo ""
    bold "Planned steps for this system:"
    echo "- Download gaze, gaze-gui, and gaze-gnome-extension packages"
    echo "- Install them with pacman"
    echo "- Configure the pacman repository for future updates"
    echo "- Enable the Gaze daemon"
fi

prompt_continue

# ── download + install standalone packages ───────────────────────────────────

BASE_URL="${GITHUB}/releases/download/${VERSION}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if is_deb; then
    DEB_ARCH="amd64"
    [ "$PKG_ARCH" = "aarch64" ] && DEB_ARCH="arm64"
    echo ""
    bold "Step 1/4: Downloading packages"
    curl -fsSL --progress-bar -o "${TMP}/gaze.deb" "${BASE_URL}/gaze_${V}_${DEB_ARCH}.deb"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gui.deb" "${BASE_URL}/gaze-gui_${V}_${DEB_ARCH}.deb"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gnome-extension.deb" "${BASE_URL}/gaze-gnome-extension_${V}_${DEB_ARCH}.deb"
    bold "Step 2/4: Installing packages"
    apt-get install -y "${TMP}/gaze.deb" "${TMP}/gaze-gui.deb" "${TMP}/gaze-gnome-extension.deb"
    bold "Step 3/4: Configuring apt repository for future updates"
    curl -fsSL "${PKG_BASE_URL}/PACKAGE-SIGNING-KEY.asc" \
        | gpg --dearmor \
        | tee /usr/share/keyrings/gundulabs-packages.gpg >/dev/null 2>&1 || true
    echo "deb [signed-by=/usr/share/keyrings/gundulabs-packages.gpg] ${PKG_BASE_URL}/deb stable main" \
        | tee /etc/apt/sources.list.d/gaze.list >/dev/null 2>&1 || true
    apt-get update -qq >/dev/null 2>&1 || true

elif is_rpm; then
    echo ""
    bold "Step 1/4: Downloading packages"
    curl -fsSL --progress-bar -o "${TMP}/gaze.rpm" "${BASE_URL}/gaze-${V}-1.${PKG_ARCH}.rpm"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gui.rpm" "${BASE_URL}/gaze-gui-${V}-1.${PKG_ARCH}.rpm"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gnome-extension.rpm" "${BASE_URL}/gaze-gnome-extension-${V}-1.${PKG_ARCH}.rpm"

    bold "Step 2/4: Installing packages"
    if command -v dnf >/dev/null 2>&1; then
        dnf install -y "${TMP}/gaze.rpm" "${TMP}/gaze-gui.rpm" "${TMP}/gaze-gnome-extension.rpm"
    else
        rpm -Uvh "${TMP}/gaze.rpm" "${TMP}/gaze-gui.rpm" "${TMP}/gaze-gnome-extension.rpm"
    fi

    if command -v authselect >/dev/null 2>&1; then
        authselect select vendor/gaze --force || true
    fi

    bold "Step 3/4: Configuring dnf repository for future updates"
    rpm --import "${PKG_BASE_URL}/PACKAGE-SIGNING-KEY.asc" >/dev/null 2>&1 || true
    tee /etc/yum.repos.d/gaze.repo >/dev/null 2>&1 <<EOF || true
[gaze]
name=Gundu Labs Packages
baseurl=${PKG_BASE_URL}/rpm/${PKG_ARCH}
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=${PKG_BASE_URL}/PACKAGE-SIGNING-KEY.asc
EOF

elif is_arch; then
    echo ""
    bold "Step 1/4: Downloading packages"
    curl -fsSL --progress-bar -o "${TMP}/gaze.pkg.tar.zst" "${BASE_URL}/gaze-${V}-1-${PKG_ARCH}.pkg.tar.zst"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gui.pkg.tar.zst" "${BASE_URL}/gaze-gui-${V}-1-${PKG_ARCH}.pkg.tar.zst"
    curl -fsSL --progress-bar -o "${TMP}/gaze-gnome-extension.pkg.tar.zst" "${BASE_URL}/gaze-gnome-extension-${V}-1-${PKG_ARCH}.pkg.tar.zst"
    bold "Step 2/4: Installing packages"
    pacman -U --noconfirm "${TMP}/gaze.pkg.tar.zst" "${TMP}/gaze-gui.pkg.tar.zst" "${TMP}/gaze-gnome-extension.pkg.tar.zst"
    bold "Step 3/4: Configuring pacman repository for future updates"
    curl -fsSL "${PKG_BASE_URL}/PACKAGE-SIGNING-KEY.asc" -o "${TMP}/gundulabs-packages.asc" 2>/dev/null || true
    if [ -f "${TMP}/gundulabs-packages.asc" ]; then
        pacman-key --add "${TMP}/gundulabs-packages.asc" >/dev/null 2>&1 || true
        KEY_FP="$(gpg --show-keys --with-colons "${TMP}/gundulabs-packages.asc" 2>/dev/null | awk -F: '/^fpr:/ {print $10; exit}')"
        [ -n "$KEY_FP" ] && pacman-key --lsign-key "$KEY_FP" >/dev/null 2>&1 || true
    fi
    tee /etc/pacman.d/gaze-mirrorlist >/dev/null 2>&1 <<EOF || true
Server = ${PKG_BASE_URL}/arch/${PKG_ARCH}
EOF
    if ! grep -q '\[gaze\]' /etc/pacman.conf 2>/dev/null; then
        tee -a /etc/pacman.conf >/dev/null 2>&1 <<'EOF' || true
[gaze]
SigLevel = Required DatabaseOptional
Include = /etc/pacman.d/gaze-mirrorlist
EOF
    fi
fi

bold "Step 4/4: Enabling Gaze daemon"
systemctl enable --now gazed 2>/dev/null || true

# ── done ─────────────────────────────────────────────────────────────────────

echo ""
green "Gaze installed successfully!"
echo ""
echo "  Enroll your face:    gaze add-face <name>"
echo "  Test authentication: gaze auth"
echo "  GUI:                 gaze-gui"
echo ""
echo "Docs: https://gaze.gundulabs.com"
