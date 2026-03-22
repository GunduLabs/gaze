#!/bin/sh
# Gaze installer - https://gaze.gundulabs.com/install.sh
# Usage: curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
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

if is_deb; then
    echo "Detected platform: Debian/Ubuntu (${PKG_ARCH})"
    echo "Package manager: apt"
    echo ""
    bold "Planned steps for this system:"
    echo "- Configure the apt repository"
    echo "- Install gaze, gaze-gui, and gaze-gnome-extension"
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
    echo "- Configure the dnf repository"
    echo "- Install gaze, gaze-gui, and gaze-gnome-extension"
    echo "- Set up the PAM modules through authselect if available"
    echo "- Enable the Gaze daemon"
elif is_arch; then
    echo "Detected platform: Arch/Manjaro (${PKG_ARCH})"
    echo "Package manager: AUR helper (yay/paru)"
    echo ""
    bold "Planned steps for this system:"
    echo "- Install gaze-bin, gaze-gui-bin, and gaze-gnome-extension-bin from the AUR"
    echo "- Enable the Gaze daemon"
fi

prompt_continue

# ── configure repositories + install packages ────────────────────────────────
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if is_deb; then
    echo ""
    bold "Step 1/4: Configuring apt repository"
    mkdir -p -m 0755 /usr/share/keyrings
    curl -fsSL "${PKG_BASE_URL}/keys/gundulabs-repo.gpg" \
        -o /usr/share/keyrings/gundulabs-archive-keyring.gpg
    curl -fsSL "${PKG_BASE_URL}/setup/deb/gundulabs.list" \
        -o /etc/apt/sources.list.d/gundulabs.list

    bold "Step 2/4: Updating package index"
    apt-get update

    bold "Step 3/4: Installing packages"
    apt-get install -y gaze gaze-gui gaze-gnome-extension

    bold "Step 4/4: Enabling Gaze daemon"
    systemctl enable --now gazed 2>/dev/null || true

elif is_rpm; then
    echo ""
    bold "Step 1/4: Configuring dnf repository"
    rpm --import "${PKG_BASE_URL}/keys/gundulabs-repo.asc"
    curl -fsSL "${PKG_BASE_URL}/setup/rpm/gundulabs.repo" -o /etc/yum.repos.d/gundulabs.repo

    bold "Step 2/4: Refreshing repository metadata"
    if command -v dnf >/dev/null 2>&1; then
        dnf makecache
    else
        yum makecache
    fi

    bold "Step 3/4: Installing packages"
    if command -v dnf >/dev/null 2>&1; then
        dnf install -y gaze gaze-gui gaze-gnome-extension
    else
        yum install -y gaze gaze-gui gaze-gnome-extension
    fi

    if command -v authselect >/dev/null 2>&1; then
        authselect select vendor/gaze --force || true
    fi

    bold "Step 4/4: Enabling Gaze daemon"
    systemctl enable --now gazed 2>/dev/null || true

elif is_arch; then
    echo ""
    bold "Step 1/3: Checking for AUR helper"
    AUR_USER="${SUDO_USER:-}"
    AUR_HELPER=""
    for helper in yay paru; do
        if command -v "$helper" >/dev/null 2>&1; then
            AUR_HELPER="$helper"
            break
        fi
        if [ -n "$AUR_USER" ] && su -c "command -v $helper" "$AUR_USER" >/dev/null 2>&1; then
            AUR_HELPER="$helper"
            break
        fi
    done

    if [ -z "$AUR_HELPER" ]; then
        red "No AUR helper found (tried: yay, paru)."
        echo ""
        echo "Gaze is distributed via the AUR and requires an AUR helper to install."
        echo "We recommend yay. To install it:"
        echo ""
        echo "  pacman -S --needed base-devel git"
        echo "  git clone https://aur.archlinux.org/yay.git"
        echo "  cd yay && makepkg -si"
        echo ""
        echo "Then re-run this installer."
        exit 1
    fi

    echo "Found AUR helper: $AUR_HELPER"

    bold "Step 2/3: Installing packages from AUR"
    if [ -n "$AUR_USER" ]; then
        su -c "$AUR_HELPER -S --noconfirm gaze-bin gaze-gui-bin gaze-gnome-extension-bin" "$AUR_USER"
    else
        red "Cannot determine the non-root user to run $AUR_HELPER."
        echo "AUR helpers must not run as root. Please install manually:"
        echo "  $AUR_HELPER -S gaze-bin gaze-gui-bin gaze-gnome-extension-bin"
        exit 1
    fi

    bold "Step 3/3: Enabling Gaze daemon"
    systemctl enable --now gazed 2>/dev/null || true
fi

# ── done ─────────────────────────────────────────────────────────────────────

echo ""
green "Gaze installed successfully!"
echo ""
echo "  Enroll your face:    gaze add-face <name>"
echo "  Test authentication: gaze auth"
echo "  GUI:                 gaze-gui"
echo ""
echo "Docs: https://gaze.gundulabs.com"
