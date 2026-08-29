#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Gaze installer: https://gaze.gundulabs.com/install.sh
# Usage: curl -fsSL https://gaze.gundulabs.com/install.sh | sh
#        curl -fsSL https://gaze.gundulabs.com/install.sh | sh -s -- --yes
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
GNOME_DOCS_URL="https://gaze.gundulabs.com/guide/gnome"
HYPRLAND_DOCS_URL="https://gaze.gundulabs.com/guide/hyprland"
KDE_DOCS_URL="https://gaze.gundulabs.com/guide/kde"
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
        cmd "auth {"
        cmd "    pam {"
        cmd "        module = hyprlock-gaze"
        cmd "    }"
        cmd "}"
        link "$HYPRLAND_DOCS_URL"
        return 0
    fi

    conf="${XDG_CONFIG_HOME:-$HOME/.config}/hypr/hyprlock.conf"
    mkdir -p "$(dirname "$conf")"

    if [ ! -f "$conf" ]; then
        cat >"$conf" <<'EOF'
auth {
    pam {
        module = hyprlock-gaze
    }
}
EOF
        ok "Created $conf with auth.pam.module = hyprlock-gaze."
        return 0
    fi

    if grep -qE '^\s*module\s*=' "$conf"; then
        current_pam="$(grep -E '^\s*module\s*=' "$conf" | head -1 | sed 's/.*=\s*//;s/\s*$//')"
        case "$current_pam" in
        hyprlock-gaze | hyprlock-gaze-simultaneous)
            ok "hyprlock.conf already uses $current_pam."
            return 0
            ;;
        *)
            warn "hyprlock.conf already sets auth.pam.module = $current_pam; leaving it."
            say "To use Gaze, change it to: module = hyprlock-gaze"
            return 0
            ;;
        esac
    fi

    cp "$conf" "$conf.gaze-backup"

    if grep -qE '^\s*pam\s*\{' "$conf"; then
        awk '
            BEGIN { done = 0 }
            /^\s*pam\s*\{/ && !done {
                print
                print "        module = hyprlock-gaze"
                done = 1
                next
            }
            { print }
        ' "$conf.gaze-backup" >"$conf"
        ok "Added module = hyprlock-gaze to existing pam {} block in $conf."
    elif grep -qE '^\s*auth\s*\{' "$conf"; then
        awk '
            BEGIN { done = 0 }
            /^\s*auth\s*\{/ && !done {
                print
                print "    pam {"
                print "        module = hyprlock-gaze"
                print "    }"
                done = 1
                next
            }
            { print }
        ' "$conf.gaze-backup" >"$conf"
        ok "Added pam { module = hyprlock-gaze } to existing auth {} block in $conf."
    else
        rm -f "$conf.gaze-backup"
        printf '\nauth {\n    pam {\n        module = hyprlock-gaze\n    }\n}\n' >>"$conf"
        ok "Appended auth { pam { module = hyprlock-gaze } } to $conf."
        return 0
    fi
    say "${DIM}Backup: $conf.gaze-backup${RESET}"
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

    # Shell does not scan newly installed system extensions until it restarts, so
    # `gnome-extensions enable` usually fails here and gsettings writes dconf directly.
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

    say "GNOME desktop session not detected; skipping the GNOME Shell extension package."
    say "CLI, GUI, and PAM modules are still installed."
    say "For non-GNOME desktop/login integration, see:"
    link "$PAM_DOCS_URL"
}

# Non-fatal: a package missing from the repo must not fail the whole install.
#
# Output stays visible. On Arch this call hands off to an AUR helper that builds from
# source and escalates through sudo, so silencing it turns a normal multi-minute build
# into what looks like a hung installer.
install_kde_packages() {
    if [ "$#" -eq 0 ]; then
        return 0
    fi
    if "$@"; then
        KDE_PACKAGES_INSTALLED=1
        return 0
    fi
    KDE_PACKAGES_INSTALLED=0
    echo ""
    warn "Could not install the KDE Plasma packages ($KDE_PKGS)."
    say "  The rest of Gaze is installed and working; only the KDE lock screen"
    say "  integration is missing. This is expected if your distribution's Gaze"
    say "  packages predate the KDE package. Retry once it is available:"
    link "$KDE_DOCS_URL"
}

# Something is already installed under the bare name `gaze`. That is either our own
# release artifact installed with `pacman -U` (unsupported: an AUR helper later
# "upgrades" it to the unrelated package below) or the unrelated AUR `gaze`, a file
# watcher. The wrapper declares conflicts=('gaze'), so pacman replaces either one
# cleanly, but which one it was changes what the user should do about it.
warn_replacing_bare_gaze_package() {
    command -v pacman >/dev/null 2>&1 || return 0
    pacman -Qq gaze >/dev/null 2>&1 || return 0

    installed_url="$(pacman -Qi gaze 2>/dev/null | awk -F ': ' '/^URL/ { print $2; exit }')"

    case "$installed_url" in
    *gundulabs*)
        warn "Gaze is installed under the bare name 'gaze' (from a release artifact)."
        say "  That name is not safe on Arch: an AUR helper treats the unrelated 'gaze'"
        say "  package as an upgrade for it. Switching you to gaze-bin, which is."
        ;;
    *)
        warn "A package named 'gaze' is already installed and is not Gaze face unlock."
        say "  That is an unrelated AUR package (a file watcher) using the same name."
        say "  Installing gaze-bin replaces it. Reinstall it afterwards if you use it."
        ;;
    esac
}

enable_kde() {
    if [ "${KDE_PACKAGES_INSTALLED:-0}" -ne 1 ]; then
        return 0
    fi
    ok "KDE Plasma lock screen face unlock: enabled by gaze-kde"
    say "  ${DIM}Face auth runs in the slot KScreenLocker starts for biometrics, alongside${RESET}"
    say "  ${DIM}the password field, so a match unlocks without pressing a key.${RESET}"
    say "  ${DIM}Lock your screen and look at the camera to try it.${RESET}"
    say "  ${DIM}Find Face Unlock in System Settings, or run gaze-gui.${RESET}"
    say "  ${DIM}The login greeter is off by default: sudo gaze-kde-pam enable-login${RESET}"
    link "$KDE_DOCS_URL"
}

enable_desktop_integrations() {
    if want_gnome_extension_package; then
        enable_gnome_extension
    elif is_kde_session; then
        enable_kde
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

    current_authselect="$(sudo authselect current 2>/dev/null || true)"

    if ! sudo test -f /etc/gaze/authselect.previous; then
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

    # authselect select resets the feature set, so carry over the features the
    # user already had (e.g. with-fingerprint) instead of silently dropping them.
    preserved_features="$(printf '%s\n' "$current_authselect" | awk '/^- /{print $2}')"

    if sudo authselect select gaze with-silent-lastlog --force >/dev/null 2>&1; then
        ok "Enabled the Gaze PAM authselect profile."
        for feature in $preserved_features; do
            [ "$feature" = "with-silent-lastlog" ] && continue
            sudo authselect enable-feature "$feature" >/dev/null 2>&1 || true
        done
    else
        warn "Could not enable the Gaze PAM authselect profile automatically."
        say "After installation, run:"
        cmd "sudo authselect select gaze with-silent-lastlog --force"
    fi
}

configure_pam_opensuse() {
    if ! command -v pam-config >/dev/null 2>&1; then
        warn "pam-config is not installed; skipping openSUSE PAM configuration."
        say "Install pam-config and re-run this installer, or configure the Gaze PAM module manually."
        link "$PAM_DOCS_URL"
        return 0
    fi

    # Query output contains auth: only when a module is active. Preserve the
    # user's selected mode when the installer runs again.
    gaze_config="$(sudo pam-config -q --gaze </dev/null 2>/dev/null || true)"
    grosshack_config="$(sudo pam-config -q --gaze_grosshack </dev/null 2>/dev/null || true)"
    gaze_active=0
    grosshack_active=0
    if printf '%s\n' "$gaze_config" | grep -qE '^[[:space:]]*auth:'; then
        gaze_active=1
    fi
    if printf '%s\n' "$grosshack_config" | grep -qE '^[[:space:]]*auth:'; then
        grosshack_active=1
    fi

    if [ "$gaze_active" -eq 1 ] || [ "$grosshack_active" -eq 1 ]; then
        if [ "$gaze_active" -eq 1 ] && [ "$grosshack_active" -eq 1 ]; then
            ok "Gaze PAM sequential and grosshack modules are already enabled through pam-config."
        elif [ "$grosshack_active" -eq 1 ]; then
            ok "Gaze PAM grosshack module is already enabled through pam-config."
        else
            ok "Gaze PAM module is already enabled through pam-config."
        fi
        return 0
    fi

    # Enable the default mode and regenerate common-*.
    if sudo pam-config --add --gaze </dev/null && sudo pam-config --update </dev/null; then
        ok "Enabled the Gaze PAM module through pam-config."
    else
        warn "Could not enable the Gaze PAM module through pam-config."
        say "After installation, run:"
        cmd "sudo pam-config --add --gaze"
        cmd "sudo pam-config --update"
        link "$PAM_DOCS_URL"
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
    if ! curl -fsSL "${PKG_BASE_URL}/keys/gundulabs-repo.asc" -o "$key_path"; then
        die "Could not download the repository signing key."
    fi
    key_info="$TMP/gundulabs-repo.key-info"
    if ! gpg --batch --show-keys --with-colons "$key_path" >"$key_info"; then
        die "Could not read the repository signing key."
    fi
    # Validate the sole primary key while allowing its subkeys.
    # The `if` preserves the mismatch error under `set -e`.
    if actual_fpr="$(awk -F: '
        $1 == "pub" { pubs++; primary = 1; next }
        primary && $1 == "fpr" { print $10; primary = 0 }
        END { if (pubs != 1) exit 1 }
    ' "$key_info")"; then
        :
    else
        actual_fpr=""
    fi
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

is_fedora_compatible() {
    case " $DISTRO_ID $DISTRO_LIKE " in
    *" fedora "*) return 0 ;;
    esac
    return 1
}

is_ostree_system() {
    [ -e /run/ostree-booted ]
}

is_opensuse_tumbleweed() {
    case "$DISTRO_ID" in
    opensuse-tumbleweed | opensuse_tumbleweed | tumbleweed) return 0 ;;
    esac
    return 1
}

is_rpm() {
    is_opensuse_tumbleweed && return 0
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
    case "$1" in
    noble | questing | resolute | trixie | forky) return 0 ;;
    esac
    return 1
}

supported_fedora_compatible_version() {
    case "$DISTRO_VERSION_ID" in
    42 | 43 | 44) return 0 ;;
    esac
    return 1
}

if ! is_rpm && ! is_deb && ! is_arch; then
    fail "Unsupported distribution: $DISTRO_ID"
    say "Supported: Ubuntu 24.04/25.10/26.04, Debian 13 and 14 (forky/testing), Fedora-compatible 42/43/44 systems (including rpm-ostree image-based distros like Silverblue, Bazzite, and Kinoite), openSUSE Tumbleweed, Arch Linux, and Arch-compatible AUR distros"
    exit 1
fi

if is_deb && ! supported_deb_suite "$DISTRO_CODENAME"; then
    for candidate in "${UBUNTU_CODENAME:-}" "${DEBIAN_CODENAME:-}"; do
        if [ -n "$candidate" ] && supported_deb_suite "$candidate"; then
            DISTRO_CODENAME="$candidate"
            break
        fi
    done
fi

if is_deb && ! supported_deb_suite "$DISTRO_CODENAME"; then
    fail "Unsupported Debian/Ubuntu release: ${DISTRO_CODENAME:-unknown}"
    say "Supported apt suites: noble, questing, resolute, trixie, forky"
    exit 1
fi

if is_rpm && ! is_fedora_compatible && ! is_opensuse_tumbleweed; then
    fail "Unsupported RPM distribution: ${NAME:-$DISTRO_ID}"
    say "This installer supports Fedora-compatible RPM distributions and openSUSE Tumbleweed."
    exit 1
fi

if is_fedora_compatible && ! supported_fedora_compatible_version; then
    fail "Unsupported ${NAME:-Fedora-compatible distribution} version: ${DISTRO_VERSION_ID:-unknown}"
    say "Fedora-compatible packages are currently available for versions 42, 43, and 44."
    exit 1
fi

if is_opensuse_tumbleweed && [ "$ARCH" != "x86_64" ]; then
    fail "Unsupported openSUSE Tumbleweed architecture: $ARCH"
    say "openSUSE Tumbleweed packages are currently available for x86_64 only."
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
        plan "Install gaze, gaze-gui, and gaze-kde (hands-free KDE lock screen face unlock)"
    else
        plan "Install gaze and gaze-gui (skip GNOME Shell extension; GNOME not detected)"
    fi
    if want_hyprlock_setup; then
        plan "Install gaze-hyprlock and configure hyprlock"
    fi
    plan "Set up the PAM modules through pam-auth-update if available"
    plan "Enable the Gaze daemon"
elif is_rpm; then
    if is_opensuse_tumbleweed; then
        need zypper
        need rpm
        RPM_TOOL="zypper"
    elif is_ostree_system && command -v rpm-ostree >/dev/null 2>&1; then
        RPM_TOOL="rpm-ostree"
    elif command -v dnf >/dev/null 2>&1; then
        RPM_TOOL="dnf"
    else
        RPM_TOOL="yum"
    fi
    say "Detected platform: ${BOLD}${NAME:-RPM-compatible distribution} ${DISTRO_VERSION_ID}${RESET} (${PKG_ARCH}), package manager: ${RPM_TOOL}"
    STEP_TOTAL=6
    echo ""
    title "This will:"
    plan "Configure the package repository"
    if want_gnome_extension_package; then
        plan "Install gaze, gaze-gui, and gaze-gnome-extension"
        plan "Enable GNOME lock screen auth for this user when possible"
    elif is_kde_session; then
        plan "Install gaze, gaze-gui, and gaze-kde (hands-free KDE lock screen face unlock)"
    else
        plan "Install gaze and gaze-gui (skip GNOME Shell extension; GNOME not detected)"
    fi
    if want_hyprlock_setup; then
        plan "Install gaze-hyprlock and configure hyprlock"
    fi
    if is_opensuse_tumbleweed; then
        plan "Enable the Gaze PAM module through pam-config if available"
    else
        plan "Enable the Gaze PAM profile through authselect if available"
    fi
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
        plan "Install gaze-bin, gaze-gui-bin, and gaze-kde-bin from the AUR (hands-free KDE lock screen face unlock)"
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
    if [ -f /etc/yum.repos.d/gundulabs.repo ] ||
        [ -f /etc/zypp/repos.d/gundulabs.repo ] ||
        [ -f /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs ]; then
        say "Refreshing repository configuration..."
        sudo rm -f /etc/yum.repos.d/gundulabs.repo /etc/zypp/repos.d/gundulabs.repo /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
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
    # Pin to the detected suite (already vetted by supported_deb_suite) so each distro gets
    # the package built against its own glibc, and to the host arch so apt skips i386.
    DEB_ARCH="$(dpkg --print-architecture)"
    printf '%s\n' "deb [arch=${DEB_ARCH} signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] ${PKG_BASE_URL}/deb ${DISTRO_CODENAME} main" |
        sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null

    step "Updating package index"
    sudo apt-get update </dev/null

    step "Installing packages"
    DEB_PKGS="gaze gaze-gui"
    if want_gnome_extension_package; then
        DEB_PKGS="$DEB_PKGS gaze-gnome-extension"
    fi
    if want_hyprlock_setup; then
        DEB_PKGS="$DEB_PKGS gaze-hyprlock"
    fi
    sudo apt-get install -y $DEB_PKGS </dev/null
    if is_kde_session; then
        KDE_PKGS="gaze-kde"
        install_kde_packages sudo apt-get install -y $KDE_PKGS </dev/null
    fi

    step "Desktop integration"
    enable_desktop_integrations

    step "Enabling Gaze daemon"
    sudo systemctl enable --now gazed </dev/null 2>/dev/null || true

elif is_rpm; then
    step "Configuring repository"
    # Verify and install the key before configuring the repository.
    KEY_PATH="$(fetch_repo_key)"
    if [ "$RPM_TOOL" != "rpm-ostree" ]; then
        if is_opensuse_tumbleweed; then
            sudo mkdir -p /etc/pki/rpm-gpg
            sudo cp "$KEY_PATH" /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
            sudo chmod 0644 /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
            sudo rpm --import "$KEY_PATH"
        else
            sudo rpm --import "$KEY_PATH" 2>/dev/null || true
        fi
    fi
    if is_opensuse_tumbleweed; then
        # Stock zypper reads repositories from /etc/zypp/repos.d.
        sudo mkdir -p /etc/zypp/repos.d
        sudo tee /etc/zypp/repos.d/gundulabs.repo >/dev/null <<EOF
[gundulabs]
name=Gundu Labs
baseurl=${PKG_BASE_URL}/rpm/opensuse/tumbleweed/\$basearch
enabled=1
autorefresh=1
type=rpm-md
gpgcheck=1
gpgkey=file:///etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
EOF
    else
        sudo mkdir -p /etc/yum.repos.d
        sudo tee /etc/yum.repos.d/gundulabs.repo >/dev/null <<EOF
[gundulabs]
name=Gundu Labs
baseurl=${PKG_BASE_URL}/rpm/fedora/\$releasever/\$basearch
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=${PKG_BASE_URL}/keys/gundulabs-repo.asc
EOF
    fi

    step "Refreshing repository metadata"
    if [ "$RPM_TOOL" = "zypper" ]; then
        sudo zypper --non-interactive refresh gundulabs </dev/null
    elif [ "$RPM_TOOL" = "rpm-ostree" ]; then
        sudo rpm-ostree refresh-md </dev/null 2>/dev/null || true
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf makecache </dev/null
    else
        sudo yum makecache </dev/null
    fi

    step "Installing packages"
    RPM_PKGS="gaze gaze-gui"
    if is_opensuse_tumbleweed; then
        # Minimal Tumbleweed installations may not include pam-config.
        RPM_PKGS="$RPM_PKGS pam-config"
    fi
    if want_gnome_extension_package; then
        RPM_PKGS="$RPM_PKGS gaze-gnome-extension"
    fi
    if want_hyprlock_setup; then
        RPM_PKGS="$RPM_PKGS gaze-hyprlock"
    fi
    if [ "$RPM_TOOL" = "rpm-ostree" ]; then
        if sudo rpm-ostree install --idempotent --apply-live $RPM_PKGS </dev/null 2>/dev/null; then
            ok "Layered and live-applied packages via rpm-ostree."
        else
            sudo rpm-ostree install --idempotent $RPM_PKGS </dev/null
            ok "Layered packages via rpm-ostree (system reboot required to activate)."
        fi
    elif [ "$RPM_TOOL" = "zypper" ]; then
        sudo zypper --non-interactive install $RPM_PKGS </dev/null
    elif command -v dnf >/dev/null 2>&1; then
        sudo dnf install -y $RPM_PKGS </dev/null
    else
        sudo yum install -y $RPM_PKGS </dev/null
    fi
    if is_kde_session; then
        KDE_PKGS="gaze-kde"
        if [ "$RPM_TOOL" = "zypper" ]; then
            install_kde_packages sudo zypper --non-interactive install $KDE_PKGS </dev/null
        elif command -v dnf >/dev/null 2>&1; then
            install_kde_packages sudo dnf install -y $KDE_PKGS </dev/null
        else
            install_kde_packages sudo yum install -y $KDE_PKGS </dev/null
        fi
    fi

    step "Configuring PAM"
    if is_opensuse_tumbleweed; then
        configure_pam_opensuse
    else
        configure_authselect
    fi

    step "Desktop integration"
    enable_desktop_integrations

    step "Enabling Gaze daemon"
    sudo systemctl enable --now gazed </dev/null 2>/dev/null || true

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
    warn_replacing_bare_gaze_package
    AUR_PKGS="gaze-bin gaze-gui-bin"
    if want_gnome_extension_package; then
        AUR_PKGS="$AUR_PKGS gaze-gnome-extension-bin"
    fi
    if want_hyprlock_setup; then
        AUR_PKGS="$AUR_PKGS gaze-hyprlock-bin"
    fi
    # --needed skips anything already at this version, so re-running the installer does
    # not rebuild from source. stdin is the piped installer itself, so close it the way
    # the apt/dnf paths do: a helper that reads a prompt from there would eat the rest of
    # this script. sudo still reads its password from the terminal.
    "$AUR_HELPER" -S --needed --noconfirm $AUR_PKGS </dev/null
    if is_kde_session; then
        KDE_PKGS="gaze-kde-bin"
        install_kde_packages "$AUR_HELPER" -S --needed --noconfirm $KDE_PKGS </dev/null
    fi

    step "Configuring PAM"
    configure_pam_arch

    step "Desktop integration"
    enable_desktop_integrations

    step "Enabling Gaze daemon"
    sudo systemctl enable --now gazed </dev/null 2>/dev/null || true
fi

# ── done ─────────────────────────────────────────────────────────────────────

printf '\n%s\n\n' "${GREEN}${BOLD}✓ Gaze installed successfully${RESET}"

# Surface problems while the user is still watching. A fresh install always warns (nothing
# enrolled, extension pending a reboot), so doctor's exit code must not abort the summary.
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
    if [ "${KDE_PACKAGES_INSTALLED:-0}" -eq 1 ]; then
        ok "KDE Plasma lock screen face unlock: gaze-kde installed"
        say "  ${DIM}Login greeter stays off until you enable it:${RESET}"
    else
        warn "KDE Plasma lock screen face unlock: gaze-kde not installed"
        say "  ${DIM}Install it to get hands-free lock screen face unlock:${RESET}"
    fi
    link "$KDE_DOCS_URL"
else
    say "  GNOME extension skipped (GNOME desktop not detected); see the PAM guide:"
    link "$PAM_DOCS_URL"
fi
if want_hyprlock_setup; then
    ok "hyprlock: configured (auth.pam.module = hyprlock-gaze)"
fi
say ""
say "Docs:   ${CYAN}https://gaze.gundulabs.com${RESET}"
say "GitHub: ${CYAN}https://github.com/GunduLabs/gaze${RESET} ${DIM}(issues and feature requests welcome)${RESET}"
