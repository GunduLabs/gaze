#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

set -eu

ALLOWED="ld-linux-aarch64.so.1
ld-linux-x86-64.so.2
libc.so.6
libgcc_s.so.1
libm.so.6"

status=0

for module in "$@"; do
    if [ ! -f "$module" ]; then
        echo "check-pam-link: $module has not been built" >&2
        exit 1
    fi

    if ! head -c 4 "$module" | grep -q 'ELF'; then
        echo "check-pam-link: skipping $module, not an ELF object"
        continue
    fi

    needed=$(objdump -p "$module" | awk '/NEEDED/ { print $2 }' | sort -u)

    for lib in $needed; do
        # The main PAM module unseals the GNOME Keyring credential locally
        # after face verification. The compatibility shim does not use TPM.
        case "${module##*/}:$lib" in
            libpam_gaze.so:libtss2-esys.so.0|libpam_gaze.so:libtss2-mu.so.0|libpam_gaze.so:libtss2-tctildr.so.0)
                continue ;;
        esac
        if ! echo "$ALLOWED" | grep -qxF "$lib"; then
            echo "check-pam-link: $module links $lib" >&2
            status=1
        fi
    done

    echo "check-pam-link: $module -> $(echo "$needed" | tr '\n' ' ')"
done

if [ "$status" -ne 0 ]; then
    cat >&2 <<'EOF'

A PAM module gained a shared-library dependency outside the allowlist.
Gaze's PAM modules use D-Bus for face verification; the main module also
uses TPM libraries for local keyring credential unsealing. Neither module
may link the camera, inference, or GUI stack. Check whether a new `use` pulled
gaze-vision in, or a crate that pam-gaze depends on grew a heavy dependency.
EOF
    exit 1
fi
