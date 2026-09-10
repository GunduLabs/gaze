#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Exercise the shipped auth control flow through real Linux-PAM in a private confdir.
# No root, camera, daemon, real credentials, or changes to /etc/pam.d are needed.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
test_dir=$(mktemp -d)
trap 'rm -r -- "$test_dir"' EXIT

if [ "$(uname -s)" != Linux ] || ! command -v cc >/dev/null 2>&1; then
    echo 'SKIP: the GDM PAM harness needs Linux and a C compiler.'
    exit 0
fi
if ! printf '#include <security/pam_appl.h>\n#include <security/pam_modules.h>\nint main(void){pam_handle_t *h=0;return pam_start_confdir("x","y",0,".",&h);}\n' \
        | cc -x c - -lpam -o "$test_dir/probe" 2>/dev/null; then
    echo 'SKIP: needs libpam headers with pam_start_confdir (Linux-PAM >= 1.4).'
    exit 0
fi

cc -Wall -Wextra -Werror -fPIC -shared -DGAZE_MOCK_MODULE \
    "$repo/scripts/keyring-pam-harness.c" -lpam -o "$test_dir/mock.so"
cc -Wall -Wextra -Werror "$repo/scripts/keyring-pam-harness.c" -lpam -o "$test_dir/driver"

for template in "$repo"/packaging/pam/gdm-face{,.arch,.deb,.suse}; do
    auth_keyring=$(sed -n '/^auth.*pam_gnome_keyring\.so/p' "$template")
    test "$auth_keyring" = "${auth_keyring//auto_start/}"
    case "$auth_keyring" in *use_authtok*) ;; *) exit 1 ;; esac
    for result in 0 7 9 25; do
        for token in token absent; do
            marker="$test_dir/called"
            rm -f -- "$marker"
            # Replace only module implementations and unrelated distro includes. Preserve
            # the shipped order and controls, including pam_deny's real implementation.
            sed -n '/^auth/p' "$template" |
                sed -e 's/pam_env.so/pam_permit.so/' \
                    -e '/[[:space:]]include[[:space:]]/d' \
                    -e "s|pam_gaze.so|$test_dir/mock.so gaze $result $token|" \
                    -e "s|pam_gnome_keyring.so.*|$test_dir/mock.so keyring $marker $token|" \
                > "$test_dir/gdm-face"
            expected=failure
            if [ "$result" -eq 0 ]; then expected=success; fi
            "$test_dir/driver" "$test_dir" "$expected"
            if [ "$expected" = success ]; then
                test "$(< "$marker")" = valid
            else
                test ! -e "$marker"
            fi
        done
    done
done
echo 'PASS: 32 GDM PAM cases; only biometric success reaches keyring, with or without a token.'
