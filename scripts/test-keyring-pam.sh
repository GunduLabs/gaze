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

# greetd has no token-only PAM service: one stack serves both a face match and a typed
# password, and pam_gaze.so reaches it through `auth substack system-auth`. There is no
# pam_deny gate, so the keyring line is reached on every login rather than only the biometric
# one. What has to hold is that it is handed a token exactly when Gaze released one; the
# conversation verdict is not asserted, because pam_gnome_keyring skips when there is no token
# instead of failing the login.
#
# This is Fedora's shipped greetd stack, with the one documented edit applied: `use_authtok`
# added to the keyring line that is already there. It is quoted verbatim otherwise, disabled
# lines included, so the harness exercises the leading-dash forms and the trailing
# `auth include postlogin` that a real file carries.
cat > "$test_dir/greetd.stack" <<'STACK'
auth       substack    system-auth
auth       optional    pam_gnome_keyring.so use_authtok
-auth       optional    pam_kwallet5.so
-auth       optional    pam_kwallet.so
auth       include     postlogin
session    optional    pam_gnome_keyring.so auto_start
STACK

auth_keyring=$(sed -n '/^auth.*pam_gnome_keyring\.so/p' "$test_dir/greetd.stack")
test "$auth_keyring" = "${auth_keyring//auto_start/}"
case "$auth_keyring" in *use_authtok*) ;; *) exit 1 ;; esac
test "$(sed -n '/^session.*pam_gnome_keyring\.so/p' "$test_dir/greetd.stack")" = \
    "session    optional    pam_gnome_keyring.so auto_start"


greetd_cases=0
greetd_gate_cases=0
for result in 0 7 9 25; do
    for token in token absent; do
        marker="$test_dir/called"
        rm -f -- "$marker"
        # `required` stands in for the substack: like a substack it lets the stack carry on to
        # the keyring line, which is what greetd's real stack does on a rejected password.
        sed -n '/^auth/p' "$test_dir/greetd.stack" |
            sed -e "s|pam_gnome_keyring.so.*|$test_dir/mock.so keyring $marker $token|" \
                -e "s|substack[[:space:]]*system-auth|required $test_dir/mock.so gaze $result $token|" \
            > "$test_dir/gdm-face"
        "$test_dir/driver" "$test_dir" ignored || true
        # A released token only ever appears when the biometric result succeeded. With
        # `absent` the daemon sets nothing and the mock scores a null token valid, so those
        # cases pin the no-token path instead of the gate; only `token` discriminates.
        if [ "$result" -eq 0 ] || [ "$token" = absent ]; then expected=valid; else expected=invalid; fi
        test "$(< "$marker")" = "$expected"
        greetd_cases=$((greetd_cases + 1))
        if [ "$token" = token ]; then greetd_gate_cases=$((greetd_gate_cases + 1)); fi
    done
done

echo 'PASS: 32 GDM PAM cases; only biometric success reaches keyring, with or without a token.'
echo "PASS: $greetd_cases greetd PAM cases, $greetd_gate_cases of which discriminate the gate (the rest pin the no-token path); the keyring is handed a token only when Gaze released one."
