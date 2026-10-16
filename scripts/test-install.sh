#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Run as root in a container: scripts/test-install.sh deb|fedora|opensuse|arch|unsupported
set -eu

installer="$(cd "$(dirname "$0")/.." && pwd)/docs/public/install.sh"
family="${1:?usage: $0 deb|fedora|opensuse|arch|unsupported}"
stubs="$(mktemp -d)"
trap 'rm -rf "$stubs"' EXIT
failures=0
runs=0

run_installer() {
    desktop="$1"
    output="$(env XDG_CURRENT_DESKTOP="$desktop" XDG_SESSION_DESKTOP= DESKTOP_SESSION= \
        NO_COLOR=1 PATH="$stubs:$PATH" sh "$installer" --dry-run 2>&1)" && status=0 || status=$?
}

report() {
    failures=$((failures + 1))
    printf 'FAIL [%s %s]: %s\n' "$family" "${desktop:-no desktop}" "$1"
    printf '%s\n' "$output" | sed 's/^/    | /'
}

expect() {
    desktop="$1"
    shift
    runs=$((runs + 1))
    run_installer "$desktop"
    if [ "$status" -ne 0 ]; then
        report "exited with status $status"
        return
    fi
    for line in "$@" "Nothing was changed."; do
        printf '%s\n' "$output" | grep -qxF -- "$line" || report "missing line: $line"
    done
    if [ "$#" -lt 3 ] && printf '%s\n' "$output" | grep -q '^  Install (optional):'; then
        report "unexpected optional install"
    fi
}

expect_failure() {
    desktop="$1"
    message="$2"
    runs=$((runs + 1))
    run_installer "$desktop"
    if [ "$status" -eq 0 ]; then
        report "succeeded, expected a failure"
        return
    fi
    printf '%s\n' "$output" | grep -qF -- "$message" || report "missing message: $message"
    if printf '%s\n' "$output" | grep -qxF "Nothing was changed."; then
        report "reached the dry-run summary"
    fi
}

stub() {
    printf '#!/bin/sh\nexit 0\n' >"$stubs/$1"
    chmod +x "$stubs/$1"
}

desktops() {
    tool="$1"
    cmd="$2"
    base="$3"
    ext="$4"
    expect GNOME "  Package manager: $tool" "  Install: $cmd $base gaze-gnome-extension$ext"
    expect X-Cinnamon "  Package manager: $tool" "  Install: $cmd $base gaze-cinnamon-extension$ext"
    expect KDE "  Package manager: $tool" "  Install: $cmd $base" "  Install (optional): ${5:-$cmd} gaze-kde$ext"
    expect Hyprland "  Package manager: $tool" "  Install: $cmd $base gaze-hyprlock$ext"
    expect sway "  Package manager: $tool" "  Install: $cmd $base"
}

case "$family" in
deb)
    desktops apt "sudo apt-get install -y" "gaze gaze-gui" ""
    ;;
fedora)
    desktops dnf "sudo dnf install -y" "gaze gaze-gui" ""
    stub rpm-ostree
    touch /run/ostree-booted
    desktops rpm-ostree "sudo rpm-ostree install --idempotent --apply-live" "gaze gaze-gui" "" \
        "sudo rpm-ostree install --idempotent"
    rm -f /run/ostree-booted
    ;;
opensuse)
    desktops zypper "sudo zypper --non-interactive install" "gaze gaze-gui pam-config" ""
    ;;
arch)
    expect_failure GNOME "No AUR helper found (tried: yay, paru)."
    stub paru
    desktops paru "paru -S --needed --noconfirm" "gaze-bin gaze-gui-bin" "-bin"
    stub yay
    expect GNOME "  Package manager: yay" "  Install: yay -S --needed --noconfirm gaze-bin gaze-gui-bin gaze-gnome-extension-bin"
    ;;
unsupported)
    expect_failure GNOME "Unsupported"
    ;;
*)
    echo "unknown family: $family" >&2
    exit 2
    ;;
esac

if [ "$failures" -ne 0 ]; then
    printf '%s of %s installer runs failed\n' "$failures" "$runs"
    exit 1
fi
printf 'All %s installer runs passed\n' "$runs"
