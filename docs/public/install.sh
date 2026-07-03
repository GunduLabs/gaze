#!/bin/sh
# Gaze installer: https://gaze.gundulabs.com/install.sh
# Usage: curl -fsSL https://gaze.gundulabs.com/install.sh | sh
#        curl -fsSL https://gaze.gundulabs.com/install.sh | sh -s -- --yes
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
GNOME_DOCS_URL="https://gaze.gundulabs.com/guide/gnome"
HYPRLAND_DOCS_URL="https://gaze.gundulabs.com/guide/hyprland"
PAM_DOCS_URL="https://gaze.gundulabs.com/guide/pam"
REPO_KEY_FPR="505AC1C71AFEDBD5555235F6CB4FA24E5C1C7C98"
AUTO_YES=0

ESC="$(printf '\033')"
if [ -t 1 ] && [ "${TERM:-}" != "dumb" ] && [ -z "${NO_COLOR:-}" ]; then
    BOLD="${ESC}[1m" DIM="${ESC}[2m" RED="${ESC}[31m" GREEN="${ESC}[32m"
    YELLOW="${ESC}[33m" CYAN="${ESC}[36m" RESET="${ESC}[0m"
else
    BOLD="" DIM="" RED="" GREEN="" YELLOW="" CYAN="" RESET=""
fi

say() { printf '%s\n' "$*"; }
title() { printf '%s\n' "${BOLD}$*${RESET}"; }
ok() { printf '%s\n' "${GREEN}✓${RESET} $*"; }
warn() { printf '%s\n' "${YELLOW}!${RESET} $*"; }
fail() { printf '%s\n' "${RED}error:${RESET} $*" >&2; }
die() {
    fail "$@"
    exit 1
}
link() { printf '%s\n' "  ${CYAN}$*${RESET}"; }
cmd() { printf '  %s\n' "$*"; }

STEP_NO=0
STEP_TOTAL=0
step() {
    STEP_NO=$((STEP_NO + 1))
    printf '\n%s\n' "${BOLD}${GREEN}==>${RESET}${BOLD} [${STEP_NO}/${STEP_TOTAL}] $*${RESET}"
}

usage() {
    cat <<'EOF'
Gaze installer

Usage:
  sh install.sh [options]

Options:
  -y, --yes                  Use detected defaults without prompting
  -h, --help                 Show this help

The GNOME extension package is installed only when a GNOME desktop session is
detected. On KDE Plasma and other desktops, the installer skips GNOME-specific
packages so it does not pull in GNOME Shell. When run from GNOME as your normal
user, it also enables lock screen face unlock for that user. GDM loads the
extension by default, but GDM login face auth is not enabled unless you
explicitly run the docs command for it.
EOF
}

while [ "$#" -gt 0 ]; do
    case "$1" in
    -y | --yes) AUTO_YES=1 ;;
    -h | --help)
        usage
        exit 0
        ;;
    *) die "Unknown option: $1" ;;
    esac
    shift
done

need() {
    command -v "$1" >/dev/null 2>&1 || die "'$1' is required but not installed."
}

prompt_continue() {
    if [ "$AUTO_YES" -eq 1 ]; then
        return 0
    fi

    echo ""
    printf '%s' "${BOLD}Continue? [y/N]:${RESET} "
    if [ -r /dev/tty ]; then
        read -r answer </dev/tty
    else
        fail "No interactive terminal available. Re-run with --yes for non-interactive install."
        exit 1
    fi

    case "$answer" in
    y | Y | yes | YES) return 0 ;;
    *)
        say "Aborted."
        exit 0
        ;;
    esac
}

is_gnome_session() {
    case "${XDG_CURRENT_DESKTOP:-}:${XDG_SESSION_DESKTOP:-}:${DESKTOP_SESSION:-}" in
    *GNOME* | *gnome*) return 0 ;;
    esac
    return 1
}

is_kde_session() {
    case "${XDG_CURRENT_DESKTOP:-}:${XDG_SESSION_DESKTOP:-}:${DESKTOP_SESSION:-}" in
    *KDE* | *kde* | *Plasma* | *plasma*) return 0 ;;
    esac
    return 1
}

want_gnome_extension_package() {
    is_gnome_session
}

is_hyprland_session() {
    case "${XDG_CURRENT_DESKTOP:-}:${XDG_SESSION_DESKTOP:-}:${DESKTOP_SESSION:-}" in
    *Hyprland* | *hyprland*) return 0 ;;
    esac
    return 1
}

has_hyprlock() {
    command -v hyprlock >/dev/null 2>&1
}

want_hyprlock_setup() {
    is_hyprland_session || has_hyprlock
}

print_manual_gnome_enable() {
    cmd "gnome-extensions enable gaze@gundulabs.com"
    cmd "gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true"
}

configure_hyprlock_conf() {
    if [ "$(id -u)" -eq 0 ]; then
        warn "Running as root; skipping per-user hyprlock.conf edit."
        say "As your desktop user, add to ~/.config/hypr/hyprlock.conf:"
        cmd "general { pam_module = hyprlock-gaze }"
        link "$HYPRLAND_DOCS_URL"
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
        ok "Created $conf with pam_module = hyprlock-gaze."
        return 0
    fi

    if grep -qE '^\s*pam_module\s*=' "$conf"; then
        current_pam="$(grep -E '^\s*pam_module\s*=' "$conf" | head -1 | sed 's/.*=\s*//;s/\s*$//')"
        case "$current_pam" in
        hyprlock-gaze | hyprlock-gaze-simultaneous)
            ok "hyprlock.conf already uses $current_pam."
            return 0
            ;;
        *)
            warn "hyprlock.conf already sets pam_module = $current_pam; leaving it."
            say "To use Gaze, change it to: pam_module = hyprlock-gaze"
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
        ok "Added pam_module = hyprlock-gaze to existing general {} block in $conf."
        say "${DIM}Backup: $conf.gaze-backup${RESET}"
    else
        printf '\ngeneral {\n    pam_module = hyprlock-gaze\n}\n' >>"$conf"
        ok "Appended general { pam_module = hyprlock-gaze } to $conf."
    fi
}

enable_hyprlock() {
    if ! want_hyprlock_setup; then
        return 0
    fi
    say "Hyprland/hyprlock detected; configuring hyprlock to use Gaze face unlock..."
    configure_hyprlock_conf
}

_gsettings_add_extension() {
    ext_id="$1"
    if ! command -v gsettings >/dev/null 2>&1; then
        return 1
    fi
    current=$(gsettings get org.gnome.shell enabled-extensions 2>/dev/null) || return 1
    case "$current" in
    *"$ext_id"*) return 0 ;;
    "@as []" | "[]") gsettings set org.gnome.shell enabled-extensions "['$ext_id']" ;;
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
        warn "Running as root; not changing per-user GNOME extension settings."
        say "For GNOME lock screen face unlock, reboot, then run as your desktop user:"
        print_manual_gnome_enable
        return 0
    fi

    if ! is_gnome_session; then
        warn "GNOME desktop session not detected; leaving the extension disabled for this user."
        say "For GNOME lock screen face unlock, reboot, then from your GNOME session:"
        print_manual_gnome_enable
        return 0
    fi

    EXT_ID="gaze@gundulabs.com"

    # gnome-extensions enable works immediately when Shell already knows the extension.
    # Newly installed system extensions are not scanned until Shell restarts, so it
    # often fails on first install. Fall back to gsettings which writes directly to
    # dconf and takes effect on the next login without needing Shell to know the ext.
    if command -v gnome-extensions >/dev/null 2>&1 && gnome-extensions enable "$EXT_ID" >/dev/null 2>&1 && _gsettings_enable_face_auth; then
        ok "Enabled GNOME lock screen face unlock for this user."
    elif _gsettings_add_extension "$EXT_ID" && _gsettings_enable_face_auth; then
        ok "Registered GNOME lock screen face unlock via dconf; a reboot will activate it."
        say "${DIM}Note: 'gnome-extensions enable $EXT_ID' before that reboot reports \"Extension does not exist\"; the dconf entry just written makes that step unnecessary.${RESET}"
    else
        warn "Could not enable the GNOME extension automatically."
        say "Reboot, then from your GNOME session run:"
        print_manual_gnome_enable
    fi
}

explain_gnome_extension_skipped() {
    if want_gnome_extension_package; then
        return 0
    fi

    if is_kde_session; then
        say "KDE Plasma desktop detected; skipping the GNOME Shell extension package."
    else
        say "GNOME desktop session not detected; skipping the GNOME Shell extension package."
    fi
    say "CLI, GUI, and PAM modules are still installed."
    say "For non-GNOME desktop/login integration, see:"
    link "$PAM_DOCS_URL"
}

enable_desktop_integrations() {
    if want_gnome_extension_package; then
        enable_gnome_extension
    else
        explain_gnome_extension_skipped
    fi
    enable_hyprlock
}

configure_pam_arch() {
    pam_file=/etc/pam.d/sudo

    if ! [ -f "$pam_file" ]; then
        warn "Could not find $pam_file; skipping PAM configuration."
        say "To enable Gaze for sudo manually, see:"
        link "$PAM_DOCS_URL"
        return 0
    fi

    if grep -q "pam_gaze" "$pam_file" 2>/dev/null; then
        ok "Gaze already configured in $pam_file."
        return 0
    fi

    awk '
        /^[[:space:]]*auth[[:space:]]/ && !done {
            print "auth        sufficient    pam_gaze.so"
            done = 1
        }
        { print }
    ' "$pam_file" >"$TMP/pam-sudo" &&
        sudo install -m 644 "$TMP/pam-sudo" "$pam_file" && {
        ok "Configured $pam_file to use Gaze face authentication."
        sudo mkdir -p /etc/gaze
        printf '%s\n' "$pam_file" | sudo tee /etc/gaze/pam-arch.configured >/dev/null
    } || {
        warn "Could not configure PAM for sudo automatically."
        say "To enable Gaze for sudo, add before the auth line in $pam_file:"
        cmd "auth    sufficient    pam_gaze.so"
        link "$PAM_DOCS_URL"
    }
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
            if printf '%s\n' "$current_authselect" >"$TMP/authselect.previous" &&
                sudo mkdir -p /etc/gaze &&
                sudo cp "$TMP/authselect.previous" /etc/gaze/authselect.previous; then
                :
            fi
            ;;
        esac
    fi

    if sudo authselect select gaze with-silent-lastlog --force >/dev/null 2>&1; then
        ok "Enabled the Gaze PAM authselect profile."
    else
        warn "Could not enable the Gaze PAM authselect profile automatically."
        say "After installation, run:"
        cmd "sudo authselect select gaze with-silent-lastlog --force"
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
    curl -fsSL "${PKG_BASE_URL}/keys/gundulabs-repo.asc" -o "$key_path"
    actual_fpr="$(gpg --show-keys --with-colons "$key_path" | awk -F: '$1 == "fpr" { print $10; exit }')"
    if [ "$actual_fpr" != "$REPO_KEY_FPR" ]; then
        fail "Repository signing key fingerprint mismatch."
        fail "Expected: $REPO_KEY_FPR"
        fail "Actual:   ${actual_fpr:-unknown}"
        exit 1
    fi
    printf '%s\n' "$key_path"
}

title "Gaze installer"
echo ""

# ── architecture ──────────────────────────────────────────────────────────────

ARCH="$(uname -m)"
case "$ARCH" in
x86_64) PKG_ARCH="x86_64" ;;
aarch64) PKG_ARCH="aarch64" ;;
*) die "Unsupported architecture: $ARCH" ;;
esac

# ── distro detection ──────────────────────────────────────────────────────────

if [ ! -f /etc/os-release ]; then
    die "Cannot detect Linux distribution (missing /etc/os-release)"
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
    *fedora* | *rhel* | *centos* | *rocky* | *alma*) return 0 ;;
    esac
    return 1
}

is_deb() {
    case "$DISTRO_ID $DISTRO_LIKE" in
    *debian* | *ubuntu*) return 0 ;;
    esac
    return 1
}

is_arch() {
    case "$DISTRO_ID $DISTRO_LIKE" in
    *arch* | *manjaro*) return 0 ;;
    esac
    return 1
}

supported_deb_suite() {
    case "$DISTRO_CODENAME" in
    noble | questing | resolute | trixie) return 0 ;;
    esac
    return 1
}

supported_fedora_version() {
    case "$DISTRO_VERSION_ID" in
    42 | 43 | 44) return 0 ;;
    esac
    return 1
}

if ! is_rpm && ! is_deb && ! is_arch; then
    fail "Unsupported distribution: $DISTRO_ID"
    say "Supported: Ubuntu 24.04/25.10/26.04, Debian 13, Fedora 42/43/44, Arch Linux, and Arch-compatible AUR distros"
    exit 1
fi

if is_deb && ! supported_deb_suite; then
    fail "Unsupported Debian/Ubuntu release: ${DISTRO_CODENAME:-unknown}"
    say "Supported apt suites: noble, questing, resolute, trixie"
    exit 1
fi

if is_rpm && ! is_fedora; then
    fail "Unsupported RPM distribution: $DISTRO_ID"
    say "Supported RPM distribution: Fedora"
    exit 1
fi

if is_fedora && ! supported_fedora_version; then
    fail "Unsupported Fedora version: ${DISTRO_VERSION_ID:-unknown}"
    say "Supported Fedora versions: 42, 43, 44"
    exit 1
fi

# ── plan ──────────────────────────────────────────────────────────────────────

plan() { printf '  %s\n' "• $*"; }

if is_deb; then
    say "Detected platform: ${BOLD}Debian/Ubuntu ${DISTRO_CODENAME}${RESET} (${PKG_ARCH}), package manager: apt"
    STEP_TOTAL=5
    echo ""
    title "This will:"
    plan "Configure the apt repository"
    if want_gnome_extension_package; then
        plan "Install gaze, gaze-gui, and gaze-gnome-extension"
        plan "Enable GNOME lock screen auth for this user when possible"
    elif is_kde_session; then
        plan "Install gaze and gaze-gui (KDE Plasma detected; skip GNOME Shell extension)"
    else
        plan "Install gaze and gaze-gui (skip GNOME Shell extension; GNOME not detected)"
    fi
    if want_hyprlock_setup; then
        plan "Install gaze-hyprlock and configure hyprlock"
    fi
    plan "Set up the PAM modules through pam-auth-update if available"
    plan "Enable the Gaze daemon"
elif is_rpm; then
    if command -v dnf >/dev/null 2>&1; then
        RPM_TOOL="dnf"
    else
        RPM_TOOL="yum"
    fi
    say "Detected platform: ${BOLD}Fedora ${DISTRO_VERSION_ID}${RESET} (${PKG_ARCH}), package manager: ${RPM_TOOL}"
    STEP_TOTAL=6
    echo ""
    title "This will:"
    plan "Configure the dnf repository"
    if want_gnome_extension_package; then
        plan "Install gaze, gaze-gui, and gaze-gnome-extension"
        plan "Enable GNOME lock screen auth for this user when possible"
    elif is_kde_session; then
        plan "Install gaze and gaze-gui (KDE Plasma detected; skip GNOME Shell extension)"
    else
        plan "Install gaze and gaze-gui (skip GNOME Shell extension; GNOME not detected)"
    fi
    if want_hyprlock_setup; then
        plan "Install gaze-hyprlock and configure hyprlock"
    fi
    plan "Enable the Gaze PAM profile through authselect if available"
    plan "Enable the Gaze daemon"
elif is_arch; then
    say "Detected platform: ${BOLD}Arch-compatible${RESET} (${PKG_ARCH}), package manager: AUR helper (yay/paru)"
    STEP_TOTAL=5
    echo ""
    title "This will:"
    if want_gnome_extension_package; then
        plan "Install gaze-bin, gaze-gui-bin, and gaze-gnome-extension-bin from the AUR"
        plan "Enable GNOME lock screen auth for this user when possible"
    elif is_kde_session; then
        plan "Install gaze-bin and gaze-gui-bin from the AUR (KDE Plasma detected; skip GNOME Shell extension)"
    else
        plan "Install gaze-bin and gaze-gui-bin from the AUR (skip GNOME Shell extension; GNOME not detected)"
    fi
    if want_hyprlock_setup; then
        plan "Install gaze-hyprlock-bin and configure hyprlock"
    fi
    plan "Configure PAM for sudo"
    plan "Enable the Gaze daemon"
fi

prompt_continue

# ── clean up old repo files ──────────────────────────────────────────────────
if is_deb; then
    if [ -f /etc/apt/sources.list.d/gundulabs.list ] || [ -f /usr/share/keyrings/gundulabs-archive-keyring.gpg ]; then
        say "Refreshing repository configuration..."
        sudo rm -f /etc/apt/sources.list.d/gundulabs.list /usr/share/keyrings/gundulabs-archive-keyring.gpg
    fi
elif is_rpm; then
    if [ -f /etc/yum.repos.d/gundulabs.repo ] || [ -f /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs ]; then
        say "Refreshing repository configuration..."
        sudo rm -f /etc/yum.repos.d/gundulabs.repo /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
    fi
fi

# ── configure repositories + install packages ────────────────────────────────
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

if is_deb; then
    step "Configuring apt repository"
    KEY_PATH="$(fetch_repo_key)"
    gpg --dearmor --yes --output "$TMP/gundulabs-archive-keyring.gpg" "$KEY_PATH"
    sudo mkdir -p -m 0755 /usr/share/keyrings
    sudo cp "$TMP/gundulabs-archive-keyring.gpg" /usr/share/keyrings/gundulabs-archive-keyring.gpg
    sudo chmod 0644 /usr/share/keyrings/gundulabs-archive-keyring.gpg
    # Pin to the detected release suite so each distro gets the package built
    # against its own toolchain/glibc (see issue #125); supported_deb_suite
    # above already guaranteed DISTRO_CODENAME is one we publish.
    printf '%s\n' "deb [signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] ${PKG_BASE_URL}/deb ${DISTRO_CODENAME} main" |
        sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null

    step "Updating package index"
    sudo apt-get update

    step "Installing packages"
    DEB_PKGS="gaze gaze-gui"
    if want_gnome_extension_package; then
        DEB_PKGS="$DEB_PKGS gaze-gnome-extension"
    fi
    if want_hyprlock_setup; then
        DEB_PKGS="$DEB_PKGS gaze-hyprlock"
    fi
    sudo apt-get install -y $DEB_PKGS

    step "Desktop integration"
    enable_desktop_integrations

    step "Enabling Gaze daemon"
    sudo systemctl enable --now gazed 2>/dev/null || true

elif is_rpm; then
    step "Configuring dnf repository"
    sudo tee /etc/yum.repos.d/gundulabs.repo >/dev/null <<EOF
[gundulabs]
name=Gundu Labs
baseurl=${PKG_BASE_URL}/rpm/fedora/\$releasever/\$basearch
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=${PKG_BASE_URL}/keys/gundulabs-repo.asc
EOF

    step "Refreshing repository metadata"
    if command -v dnf >/dev/null 2>&1; then
        sudo dnf makecache
    else
        sudo yum makecache
    fi

    step "Installing packages"
    RPM_PKGS="gaze gaze-gui"
    if want_gnome_extension_package; then
        RPM_PKGS="$RPM_PKGS gaze-gnome-extension"
    fi
    if want_hyprlock_setup; then
        RPM_PKGS="$RPM_PKGS gaze-hyprlock"
    fi
    if command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y $RPM_PKGS
    else
        sudo yum install -y $RPM_PKGS
    fi

    step "Configuring PAM"
    configure_authselect

    step "Desktop integration"
    enable_desktop_integrations

    step "Enabling Gaze daemon"
    sudo systemctl enable --now gazed 2>/dev/null || true

elif is_arch; then
    step "Checking for AUR helper"
    AUR_HELPER=""
    for helper in yay paru; do
        if command -v "$helper" >/dev/null 2>&1; then
            AUR_HELPER="$helper"
            break
        fi
    done

    if [ -z "$AUR_HELPER" ]; then
        fail "No AUR helper found (tried: yay, paru)."
        say ""
        say "Gaze is distributed via the AUR and requires an AUR helper to install."
        say "We recommend yay. To install it:"
        say ""
        cmd "sudo pacman -S --needed base-devel git"
        cmd "git clone https://aur.archlinux.org/yay.git"
        cmd "cd yay && makepkg -si"
        say ""
        say "Then re-run this installer."
        exit 1
    fi

    ok "Found AUR helper: $AUR_HELPER"

    step "Installing packages from AUR"
    AUR_PKGS="gaze-bin gaze-gui-bin"
    if want_gnome_extension_package; then
        AUR_PKGS="$AUR_PKGS gaze-gnome-extension-bin"
    fi
    if want_hyprlock_setup; then
        AUR_PKGS="$AUR_PKGS gaze-hyprlock-bin"
    fi
    "$AUR_HELPER" -S --noconfirm $AUR_PKGS

    step "Configuring PAM"
    configure_pam_arch

    step "Desktop integration"
    enable_desktop_integrations

    step "Enabling Gaze daemon"
    sudo systemctl enable --now gazed 2>/dev/null || true
fi

# ── done ─────────────────────────────────────────────────────────────────────

printf '\n%s\n\n' "${GREEN}${BOLD}✓ Gaze installed successfully${RESET}"

# Surface problems while the user is still looking at the terminal. Expect a
# few warnings on a fresh install (nothing enrolled yet, extension loads after
# a reboot); doctor's exit code must not abort the summary.
if command -v gaze >/dev/null 2>&1; then
    title "Health check (gaze doctor)"
    say "${DIM}Warnings about enrollment or the GNOME extension are expected before the next steps below.${RESET}"
    if command -v busctl >/dev/null 2>&1; then
        say "${DIM}Waiting for the daemon to finish first-run model download...${RESET}"
        i=0
        while [ "$i" -lt 80 ]; do
            if busctl --system status com.gundulabs.Gaze >/dev/null 2>&1; then
                break
            fi
            sleep 0.5
            i=$((i + 1))
        done
    fi
    gaze doctor || true
    say ""
fi

title "Next steps"
say "  1. ${BOLD}gaze config${RESET}            ${DIM}configure your camera and security settings${RESET}"
say "  2. ${BOLD}gaze add-face <name>${RESET}   ${DIM}enroll your face${RESET}"
if want_gnome_extension_package; then
    say "  3. ${BOLD}Reboot${RESET}                 ${DIM}GNOME Shell and GDM only pick up the new extension at startup${RESET}"
fi
say ""
title "Try it"
say "  ${BOLD}gaze auth${RESET}              ${DIM}test face authentication in the terminal${RESET}"
say "  ${BOLD}gaze-gui${RESET}               ${DIM}open the settings app${RESET}"
say ""
title "Desktop integration"
if want_gnome_extension_package; then
    ok "GNOME lock screen face unlock: enabled for this user (active after reboot)"
    say "  ${DIM}GDM login face auth stays off until you enable it:${RESET}"
    link "${GNOME_DOCS_URL}#optional-enable-face-at-gdm-login"
elif is_kde_session; then
    say "  KDE Plasma: GNOME extension skipped; see the PAM guide for lock/login integration:"
    link "$PAM_DOCS_URL"
else
    say "  GNOME extension skipped (GNOME desktop not detected); see the PAM guide:"
    link "$PAM_DOCS_URL"
fi
if want_hyprlock_setup; then
    ok "hyprlock: configured (pam_module = hyprlock-gaze)"
fi
say ""
say "Docs:   ${CYAN}https://gaze.gundulabs.com${RESET}"
say "GitHub: ${CYAN}https://github.com/GunduLabs/gaze${RESET} ${DIM}(issues and feature requests welcome)${RESET}"
