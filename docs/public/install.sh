#!/bin/sh
# Gaze installer - https://gaze.gundulabs.com/install.sh
# Usage: curl -fsSL https://gaze.gundulabs.com/install.sh | sh
#        curl -fsSL https://gaze.gundulabs.com/install.sh | sh -s -- --yes
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
GNOME_DOCS_URL="https://gaze.gundulabs.com/guide/gnome"
HYPRLAND_DOCS_URL="https://gaze.gundulabs.com/guide/hyprland"
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

The GNOME extension package is installed by default. When run from a GNOME
desktop session as your normal user, this installer also enables lock screen
face unlock for that user. GDM loads the extension by default, but GDM login
face auth is not enabled unless you explicitly run the docs command for it.
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
        read -r answer </dev/tty
    else
        red "No interactive terminal available. Re-run with --yes for non-interactive install."
        exit 1
    fi

    case "$answer" in
        y|Y|yes|YES) return 0 ;;
        *) echo "Aborted."; exit 0 ;;
    esac
}

is_gnome_session() {
    case "${XDG_CURRENT_DESKTOP:-}:${XDG_SESSION_DESKTOP:-}:${DESKTOP_SESSION:-}" in
        *GNOME*|*gnome*) return 0 ;;
    esac
    return 1
}

is_hyprland_session() {
    case "${XDG_CURRENT_DESKTOP:-}:${XDG_SESSION_DESKTOP:-}:${DESKTOP_SESSION:-}" in
        *Hyprland*|*hyprland*) return 0 ;;
    esac
    return 1
}

has_hyprlock() {
    command -v hyprlock >/dev/null 2>&1
}

want_hyprlock_setup() {
    is_hyprland_session || has_hyprlock
}

configure_hyprlock_conf() {
    if [ "$(id -u)" -eq 0 ]; then
        echo "Running as root; skipping per-user hyprlock.conf edit."
        echo "As your desktop user, add to ~/.config/hypr/hyprlock.conf:"
        echo "    general { pam_module = hyprlock-gaze }"
        echo "Docs: ${HYPRLAND_DOCS_URL}"
        return 0
    fi

    conf="${XDG_CONFIG_HOME:-$HOME/.config}/hypr/hyprlock.conf"
    mkdir -p "$(dirname "$conf")"

    if [ ! -f "$conf" ]; then
        cat >"$conf" <<'EOF'
general {
    pam_module = hyprlock-gaze
}
EOF
        echo "Created $conf with pam_module = hyprlock-gaze."
        return 0
    fi

    if grep -qE '^\s*pam_module\s*=' "$conf"; then
        current_pam="$(grep -E '^\s*pam_module\s*=' "$conf" | head -1 | sed 's/.*=\s*//;s/\s*$//')"
        case "$current_pam" in
            hyprlock-gaze|hyprlock-gaze-simultaneous)
                echo "hyprlock.conf already uses $current_pam."
                return 0
                ;;
            *)
                echo "hyprlock.conf already sets pam_module = $current_pam; leaving it."
                echo "To use Gaze, change it to: pam_module = hyprlock-gaze"
                return 0
                ;;
        esac
    fi

    if grep -qE '^\s*general\s*\{' "$conf"; then
        cp "$conf" "$conf.gaze-backup"
        awk '
            BEGIN { done = 0 }
            /^\s*general\s*\{/ && !done {
                print
                print "    pam_module = hyprlock-gaze"
                done = 1
                next
            }
            { print }
        ' "$conf.gaze-backup" >"$conf"
        echo "Added pam_module = hyprlock-gaze to existing general {} block in $conf."
        echo "Backup: $conf.gaze-backup"
    else
        printf '\ngeneral {\n    pam_module = hyprlock-gaze\n}\n' >>"$conf"
        echo "Appended general { pam_module = hyprlock-gaze } to $conf."
    fi
}

enable_hyprlock() {
    if ! want_hyprlock_setup; then
        return 0
    fi
    echo ""
    bold "Hyprland/hyprlock detected"
    echo "Configuring hyprlock to use Gaze face unlock..."
    configure_hyprlock_conf
    echo "Docs: ${HYPRLAND_DOCS_URL}"
}

_gsettings_add_extension() {
    ext_id="$1"
    if ! command -v gsettings >/dev/null 2>&1; then
        return 1
    fi
    current=$(gsettings get org.gnome.shell enabled-extensions 2>/dev/null) || return 1
    case "$current" in
        *"$ext_id"*) return 0 ;;
        "@as []"|"[]") gsettings set org.gnome.shell enabled-extensions "['$ext_id']" ;;
        *) gsettings set org.gnome.shell enabled-extensions "$(printf '%s' "$current" | sed "s/]$/, '$ext_id']/")" ;;
    esac
}

_gsettings_enable_face_auth() {
    if ! command -v gsettings >/dev/null 2>&1; then
        return 1
    fi
    gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
}

enable_gnome_extension() {
    if [ "$(id -u)" -eq 0 ]; then
        echo "Running as root; not changing per-user GNOME extension settings."
        echo "For GNOME lock screen face unlock, reboot, then run as your desktop user:"
        echo "  gnome-extensions enable gaze@gundulabs.com"
        echo "  gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true"
        echo "GDM loads the extension by default. Login face auth requires the docs command: ${GNOME_DOCS_URL}"
        return 0
    fi

    if ! is_gnome_session; then
        echo "GNOME desktop session not detected; leaving the extension disabled for this user."
        echo "For GNOME lock screen face unlock, reboot, then from your GNOME session:"
        echo "  gnome-extensions enable gaze@gundulabs.com"
        echo "  gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true"
        echo "GDM loads the extension by default. Login face auth requires the docs command: ${GNOME_DOCS_URL}"
        return 0
    fi

    EXT_ID="gaze@gundulabs.com"

    # gnome-extensions enable works immediately when Shell already knows the extension.
    # Newly installed system extensions are not scanned until Shell restarts, so it
    # often fails on first install. Fall back to gsettings which writes directly to
    # dconf and takes effect on the next login without needing Shell to know the ext.
    if command -v gnome-extensions >/dev/null 2>&1 && gnome-extensions enable "$EXT_ID" >/dev/null 2>&1 && _gsettings_enable_face_auth; then
        echo "Enabled GNOME lock screen face unlock for this user."
    elif _gsettings_add_extension "$EXT_ID" && _gsettings_enable_face_auth; then
        echo "Registered GNOME lock screen face unlock via dconf. Reboot to activate it."
        echo "Note: running 'gnome-extensions enable $EXT_ID' before that reboot will report \"Extension does not exist\"; the dconf entry just written makes that step unnecessary."
    else
        echo "Could not enable the GNOME extension automatically."
        echo "Reboot, then from your GNOME session run:"
        echo "  gnome-extensions enable gaze@gundulabs.com"
        echo "  gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true"
    fi

    echo "GDM loads the extension by default. Login face auth requires the docs command: ${GNOME_DOCS_URL}"
}

configure_authselect() {
    if ! command -v authselect >/dev/null 2>&1; then
        return 0
    fi

    if ! sudo test -f /etc/gaze/authselect.previous; then
        current_authselect="$(sudo authselect current 2>/dev/null || true)"
        case "$current_authselect" in
            *"Profile ID: gaze"*) ;;
            "") ;;
            *)
                if printf '%s\n' "$current_authselect" >"$TMP/authselect.previous" && \
                    sudo mkdir -p /etc/gaze && \
                    sudo cp "$TMP/authselect.previous" /etc/gaze/authselect.previous; then
                    :
                fi
                ;;
        esac
    fi

    if sudo authselect select gaze with-silent-lastlog --force >/dev/null 2>&1; then
        echo "Enabled the Gaze PAM authselect profile."
    else
        echo "Could not enable the Gaze PAM authselect profile automatically."
        echo "After installation, run:"
        echo "  sudo authselect select gaze with-silent-lastlog --force"
    fi
}

need curl
need grep
need uname
need awk
need id
need gpg

fetch_repo_key() {
    key_path="$TMP/gundulabs-repo.asc"
    curl -fsSL "${PKG_BASE_URL}/apt/gpg.key" -o "$key_path"
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
        noble|questing|resolute|trixie) return 0 ;;
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
    echo "Supported: Ubuntu 24.04/25.10/26.04, Debian 13, Fedora 42/43/44, Arch Linux, Manjaro"
    exit 1
fi

if is_deb && ! supported_deb_suite; then
    red "Unsupported Debian/Ubuntu release: ${DISTRO_CODENAME:-unknown}"
    echo "Supported apt suites: noble, questing, resolute, trixie"
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
    echo "- Install gaze, gaze-gui, gaze-gnome-extension, and gaze-hyprlock (if Hyprland detected)"
    echo "- Enable GNOME lock screen auth for this user when possible"
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
    echo "- Install gaze, gaze-gui, gaze-gnome-extension, and gaze-hyprlock (if Hyprland detected)"
    echo "- Enable GNOME lock screen auth for this user when possible"
    echo "- Enable the Gaze PAM profile through authselect if available"
    echo "- Enable the Gaze daemon"
elif is_arch; then
    echo "Detected platform: Arch/Manjaro (${PKG_ARCH})"
    echo "Package manager: AUR helper (yay/paru)"
    echo ""
    bold "Planned steps for this system:"
    echo "- Install gaze-bin, gaze-gui-bin, gaze-gnome-extension-bin, and gaze-hyprlock-bin (if Hyprland detected) from the AUR"
    echo "- Enable GNOME lock screen auth for this user when possible"
    echo "- Enable the Gaze daemon"
fi

prompt_continue

# ── clean up old repo files ──────────────────────────────────────────────────
if is_deb; then
    if [ -f /etc/apt/sources.list.d/gundulabs.list ] || [ -f /usr/share/keyrings/gundulabs-archive-keyring.gpg ]; then
        echo "Cleaning up legacy repository configuration..."
        sudo rm -f /etc/apt/sources.list.d/gundulabs.list /usr/share/keyrings/gundulabs-archive-keyring.gpg
    fi
elif is_rpm; then
    if [ -f /etc/yum.repos.d/gundulabs.repo ] || [ -f /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs ]; then
        echo "Cleaning up legacy repository configuration..."
        sudo rm -f /etc/yum.repos.d/gundulabs.repo /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
    fi
fi

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
    printf '%s\n' "deb [signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] ${PKG_BASE_URL}/apt/ * *" \
        | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null

    bold "Step 2/5: Updating package index"
    sudo apt-get update

    bold "Step 3/5: Installing packages"
    DEB_PKGS="gaze gaze-gui gaze-gnome-extension"
    if want_hyprlock_setup; then
        DEB_PKGS="$DEB_PKGS gaze-hyprlock"
    fi
    sudo apt-get install -y $DEB_PKGS

    bold "Step 4/5: Enabling GNOME lock screen auth"
    enable_gnome_extension
    enable_hyprlock

    bold "Step 5/5: Enabling Gaze daemon"
    sudo systemctl enable --now gazed 2>/dev/null || true

elif is_rpm; then
    echo ""
    bold "Step 1/5: Configuring dnf repository"
    sudo tee /etc/yum.repos.d/gundulabs.repo >/dev/null <<EOF
[gundulabs]
name=Gundu Labs
baseurl=${PKG_BASE_URL}/yum/
enabled=1
gpgcheck=1
repo_gpgcheck=0
gpgkey=${PKG_BASE_URL}/yum/gpg.key
EOF

    bold "Step 2/5: Refreshing repository metadata"
    if command -v dnf >/dev/null 2>&1; then
        sudo dnf makecache
    else
        sudo yum makecache
    fi

    bold "Step 3/5: Installing packages"
    RPM_PKGS="gaze gaze-gui gaze-gnome-extension"
    if want_hyprlock_setup; then
        RPM_PKGS="$RPM_PKGS gaze-hyprlock"
    fi
    if command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y $RPM_PKGS
    else
        sudo yum install -y $RPM_PKGS
    fi

    configure_authselect

    bold "Step 4/5: Enabling GNOME lock screen auth"
    enable_gnome_extension
    enable_hyprlock

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
    AUR_PKGS="gaze-bin gaze-gui-bin gaze-gnome-extension-bin"
    if want_hyprlock_setup; then
        AUR_PKGS="$AUR_PKGS gaze-hyprlock-bin"
    fi
    "$AUR_HELPER" -S --noconfirm $AUR_PKGS

    bold "Step 3/4: Enabling GNOME lock screen auth"
    enable_gnome_extension
    enable_hyprlock

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
echo "  GNOME lock screen:   enabled for this GNOME user when possible"
echo "  GDM extension:       enabled by package defaults"
echo "  GDM login face auth: disabled until you run the docs command"
if want_hyprlock_setup; then
    echo "  hyprlock:            configured (pam_module = hyprlock-gaze)"
fi
echo ""
bold "Optional GDM login setup docs:"
green "https://gaze.gundulabs.com/guide/gnome#optional-enable-face-at-gdm-login"
echo ""
echo "Docs: https://gaze.gundulabs.com"
