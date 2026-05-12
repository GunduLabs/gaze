#!/bin/sh
# Gaze installer - https://gaze.gundulabs.com/install.sh
# Usage: curl -fsSL https://gaze.gundulabs.com/install.sh | sh
#        curl -fsSL https://gaze.gundulabs.com/install.sh | sh -s -- --yes
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
REPO_KEY_FPR="505AC1C71AFEDBD5555235F6CB4FA24E5C1C7C98"
AUTO_YES=0

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
bold()  { printf '\033[1m%s\033[0m\n' "$*"; }

usage() {
    cat <<'EOF'
Gaze installer

Usage:
  sh install.sh [options]

Options:
  -y, --yes                  Use detected defaults without prompting
  -h, --help                 Show this help

The GNOME extension package is installed by default, but it is not enabled by
this installer. Enable it separately if you want GNOME lock screen face unlock.
GDM login face auth is also not enabled by this installer.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        -y|--yes) AUTO_YES=1 ;;
        -h|--help) usage; exit 0 ;;
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

print_gnome_extension_next_steps() {
    echo "Installed gaze-gnome-extension, but did not enable it automatically."
    echo "For GNOME lock screen face unlock, log out and back in if needed, then run:"
    echo "  gnome-extensions enable gaze@gundulabs.com"
    echo "GDM login face auth remains disabled by default. See the GNOME docs before enabling it."
}

need curl
need grep
need uname
need awk
need id
need gpg

fetch_repo_key() {
    key_path="$TMP/gundulabs-repo.asc"
    curl -fsSL "${PKG_BASE_URL}/keys/gundulabs-repo.asc" -o "$key_path"
    actual_fpr="$(gpg --show-keys --with-colons "$key_path" | awk -F: '$1 == "fpr" { print $10; exit }')"
    if [ "$actual_fpr" != "$REPO_KEY_FPR" ]; then
        red "Repository signing key fingerprint mismatch."
        red "Expected: $REPO_KEY_FPR"
        red "Actual:   ${actual_fpr:-unknown}"
        exit 1
    fi
    printf '%s\n' "$key_path"
}

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
DISTRO_VERSION_ID="${VERSION_ID:-}"
DISTRO_CODENAME="${VERSION_CODENAME:-${UBUNTU_CODENAME:-}}"
VARIANT_ID="${VARIANT_ID:-}"

is_fedora() {
    [ "$DISTRO_ID" = "fedora" ]
}

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

supported_deb_suite() {
    case "$DISTRO_CODENAME" in
        noble|resolute|trixie) return 0 ;;
    esac
    return 1
}

supported_fedora_version() {
    case "$DISTRO_VERSION_ID" in
        42|43|44) return 0 ;;
    esac
    return 1
}

if ! is_rpm && ! is_deb && ! is_arch; then
    red "Unsupported distribution: $DISTRO_ID"
    echo "Supported: Ubuntu 24.04/26.04, Debian 13, Fedora 42/43/44, Arch Linux, Manjaro"
    exit 1
fi

if is_deb && ! supported_deb_suite; then
    red "Unsupported Debian/Ubuntu release: ${DISTRO_CODENAME:-unknown}"
    echo "Supported apt suites: noble, resolute, trixie"
    exit 1
fi

if is_rpm && ! is_fedora; then
    red "Unsupported RPM distribution: $DISTRO_ID"
    echo "Supported RPM distribution: Fedora"
    exit 1
fi

if is_fedora && ! supported_fedora_version; then
    red "Unsupported Fedora version: ${DISTRO_VERSION_ID:-unknown}"
    echo "Supported Fedora versions: 42, 43, 44"
    exit 1
fi

if is_deb; then
    echo "Detected platform: Debian/Ubuntu ${DISTRO_CODENAME} (${PKG_ARCH})"
    echo "Package manager: apt"
    echo ""
    bold "Planned steps for this system:"
    echo "- Configure the apt repository"
    echo "- Install gaze, gaze-gui, and gaze-gnome-extension"
    echo "- Leave the GNOME extension disabled until you enable it"
    echo "- Set up the PAM modules through pam-auth-update if available"
    echo "- Enable the Gaze daemon"
elif is_rpm; then
    echo "Detected platform: Fedora ${DISTRO_VERSION_ID} (${PKG_ARCH})"
    if command -v dnf >/dev/null 2>&1; then
        echo "Package manager: dnf"
    else
        echo "Package manager: rpm"
    fi
    echo ""
    bold "Planned steps for this system:"
    echo "- Configure the dnf repository"
    echo "- Install gaze, gaze-gui, and gaze-gnome-extension"
    echo "- Leave the GNOME extension disabled until you enable it"
    echo "- Set up the PAM modules through authselect if available"
    echo "- Enable the Gaze daemon"
elif is_arch; then
    echo "Detected platform: Arch/Manjaro (${PKG_ARCH})"
    echo "Package manager: AUR helper (yay/paru)"
    echo ""
    bold "Planned steps for this system:"
    echo "- Install gaze-bin, gaze-gui-bin, and gaze-gnome-extension-bin from the AUR"
    echo "- Leave the GNOME extension disabled until you enable it"
    echo "- Enable the Gaze daemon"
fi

prompt_continue

# ── configure repositories + install packages ────────────────────────────────
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if is_deb; then
    echo ""
    bold "Step 1/5: Configuring apt repository"
    KEY_PATH="$(fetch_repo_key)"
    gpg --dearmor --yes --output "$TMP/gundulabs-archive-keyring.gpg" "$KEY_PATH"
    sudo mkdir -p -m 0755 /usr/share/keyrings
    sudo cp "$TMP/gundulabs-archive-keyring.gpg" /usr/share/keyrings/gundulabs-archive-keyring.gpg
    sudo chmod 0644 /usr/share/keyrings/gundulabs-archive-keyring.gpg
    printf '%s\n' "deb [signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] ${PKG_BASE_URL}/deb ${DISTRO_CODENAME} main" \
        | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null

    bold "Step 2/5: Updating package index"
    sudo apt-get update

    bold "Step 3/5: Installing packages"
    sudo apt-get install -y gaze gaze-gui gaze-gnome-extension

    bold "Step 4/5: GNOME extension next step"
    print_gnome_extension_next_steps

    bold "Step 5/5: Enabling Gaze daemon"
    sudo systemctl enable --now gazed 2>/dev/null || true

elif is_rpm; then
    echo ""
    bold "Step 1/5: Configuring dnf repository"
    KEY_PATH="$(fetch_repo_key)"
    sudo mkdir -p -m 0755 /etc/pki/rpm-gpg
    sudo cp "$KEY_PATH" /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
    sudo chmod 0644 /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
    sudo rpm --import /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
    sudo tee /etc/yum.repos.d/gundulabs.repo >/dev/null <<EOF
[gundulabs]
name=Gundu Labs
baseurl=${PKG_BASE_URL}/rpm/fedora/\$releasever/\$basearch
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
EOF

    bold "Step 2/5: Refreshing repository metadata"
    if command -v dnf >/dev/null 2>&1; then
        sudo dnf makecache
    else
        sudo yum makecache
    fi

    bold "Step 3/5: Installing packages"
    if command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y gaze gaze-gui gaze-gnome-extension
    else
        sudo yum install -y gaze gaze-gui gaze-gnome-extension
    fi

    if command -v authselect >/dev/null 2>&1; then
        sudo authselect select vendor/gaze --force || true
        sudo authselect enable-feature with-silent-lastlog || true
    fi

    bold "Step 4/5: GNOME extension next step"
    print_gnome_extension_next_steps

    bold "Step 5/5: Enabling Gaze daemon"
    sudo systemctl enable --now gazed 2>/dev/null || true

elif is_arch; then
    echo ""
    bold "Step 1/4: Checking for AUR helper"
    AUR_HELPER=""
    for helper in yay paru; do
        if command -v "$helper" >/dev/null 2>&1; then
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
        echo "  sudo pacman -S --needed base-devel git"
        echo "  git clone https://aur.archlinux.org/yay.git"
        echo "  cd yay && makepkg -si"
        echo ""
        echo "Then re-run this installer."
        exit 1
    fi

    echo "Found AUR helper: $AUR_HELPER"

    bold "Step 2/4: Installing packages from AUR"
    "$AUR_HELPER" -S --noconfirm gaze-bin gaze-gui-bin gaze-gnome-extension-bin

    bold "Step 3/4: GNOME extension next step"
    print_gnome_extension_next_steps

    bold "Step 4/4: Enabling Gaze daemon"
    sudo systemctl enable --now gazed 2>/dev/null || true
fi

# ── done ─────────────────────────────────────────────────────────────────────

echo ""
green "Gaze installed successfully!"
echo ""
echo "  Enroll your face:    gaze add-face <name>"
echo "  Test authentication: gaze auth"
echo "  GUI:                 gaze-gui"
echo "  GNOME lock screen:   gnome-extensions enable gaze@gundulabs.com"
echo "  GDM login face auth: disabled by default"
echo ""
echo "Docs: https://gaze.gundulabs.com"
