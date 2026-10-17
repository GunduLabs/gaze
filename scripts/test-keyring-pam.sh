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

mkdir "$test_dir/tallies"
export GAZE_TEST_PAM_USER
GAZE_TEST_PAM_USER=$(id -un)
faillock="pam_faillock.so dir=$test_dir/tallies deny=2 unlock_time=0 even_deny_root"

render_stack() {
    local template=$1 result=$2 token=$3 keyring=$4 drop_keyring=
    if [ "$keyring" = removed ]; then drop_keyring='/pam_gnome_keyring\.so/d'; fi
    # Keep the shipped controls and real faillock/deny modules; isolate tally files.
    sed -n '/^auth/p' "$template" |
        sed -e "${drop_keyring:-/^\$/d}" \
            -e "s|pam_env.so|$test_dir/mock.so gaze 25 absent|" \
            -e '/[[:space:]]include[[:space:]]/d' \
            -e "s|pam_gaze.so|$test_dir/mock.so gaze $result $token|" \
            -e "s|pam_faillock.so|$faillock|" \
            -e "s|pam_gnome_keyring.so.*|$test_dir/mock.so keyring $test_dir/called $token|" \
        > "$test_dir/gdm-face"
}

printf '%s\n' "auth required $faillock preauth" \
    'auth required pam_permit.so' 'session required pam_permit.so' > "$test_dir/lockout-probe"

for template in "$repo"/packaging/pam/gdm-face{,.arch,.deb,.suse}; do
    auth_keyring=$(sed -n '/^auth.*pam_gnome_keyring\.so/p' "$template")
    test "$auth_keyring" = "${auth_keyring//auto_start/}"
    case "$auth_keyring" in *use_authtok*) ;; *) exit 1 ;; esac
    for result in 0 7 9 25; do
        for token in token absent; do
            for keyring in present removed; do
                marker="$test_dir/called"
                rm -f -- "$marker"
                rm -f -- "$test_dir/tallies/"*
                render_stack "$template" "$result" "$token" "$keyring"
                expected=failure
                if [ "$result" -eq 0 ]; then expected=success; fi
                "$test_dir/driver" "$test_dir" "$expected"
                if [ "$expected" = success ] && [ "$keyring" = present ]; then
                    test "$(< "$marker")" = valid
                else
                    test ! -e "$marker"
                fi
            done
        done
    done
    # Two rejected faces lock the private account and prevent a later face success.
    rm -f -- "$test_dir/tallies/"*
    render_stack "$template" 7 absent removed
    "$test_dir/driver" "$test_dir" failure
    "$test_dir/driver" "$test_dir" failure
    "$test_dir/driver" "$test_dir" failure lockout-probe
    render_stack "$template" 0 absent removed
    "$test_dir/driver" "$test_dir" failure

    # A successful face resets the tally; non-decisions neither increment nor clear it.
    rm -f -- "$test_dir/tallies/"*
    render_stack "$template" 7 absent removed
    "$test_dir/driver" "$test_dir" failure
    render_stack "$template" 0 absent removed
    "$test_dir/driver" "$test_dir" success
    render_stack "$template" 7 absent removed
    "$test_dir/driver" "$test_dir" failure
    "$test_dir/driver" "$test_dir" success lockout-probe
    for result in 9 25; do
        rm -f -- "$test_dir/tallies/"*
        render_stack "$template" 7 absent removed
        "$test_dir/driver" "$test_dir" failure
        render_stack "$template" "$result" absent removed
        for _ in 1 2 3; do "$test_dir/driver" "$test_dir" failure; done
        "$test_dir/driver" "$test_dir" success lockout-probe
        render_stack "$template" 7 absent removed
        "$test_dir/driver" "$test_dir" failure
        "$test_dir/driver" "$test_dir" failure lockout-probe
    done
done

# greetd always reaches keyring; it must see a token exactly when Gaze released one.
# Do not assert authentication: the password-checking substack is not modelled.
for result in 0 7 9 25; do
    for token in token absent; do
        marker="$test_dir/called"
        rm -f -- "$marker"
        printf '%s\n' \
            "auth required $test_dir/mock.so gaze $result $token" \
            "auth optional $test_dir/mock.so keyring $marker $token" \
            > "$test_dir/gdm-face"
        "$test_dir/driver" "$test_dir" success || true
        expected=invalid
        if [ "$result" -eq 0 ] || [ "$token" = absent ]; then expected=valid; fi
        test "$(< "$marker")" = "$expected"
    done
done

echo 'PASS: GDM keyring handoff and face lockout; greetd token handoff.'
