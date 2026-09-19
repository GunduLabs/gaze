#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
# Private PAM files and synthetic credentials only; never edits the host's PAM stack.
set -euo pipefail
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
test_dir=$(mktemp -d)
trap 'rm -rf -- "$test_dir"' EXIT
export GAZE_KDE_PAM_FILE="$test_dir/pam/kde-fingerprint"
export GAZE_KDE_SMARTCARD_PAM_FILE="$test_dir/pam/kde-smartcard"
export GAZE_KDE_LOGIN_PAM_FILES="$test_dir/pam/sddm $test_dir/pam/plasmalogin"
export GAZE_KDE_LOGIN_FACE_PAM_FILE="$test_dir/pam/plasmalogin-fingerprint"
export GAZE_KDE_VENDOR_PAM_DIR="$test_dir/vendor"
export GAZE_KDE_STATE_DIR="$test_dir/state"
export GAZE_KDE_SECURITY_DIRS="$test_dir/modules"
mkdir -p "$test_dir"/{pam,vendor,modules}
touch "$test_dir/modules/pam_gaze.so"
helper="$repo/packaging/kde/gaze-kde-pam"
for service in sddm plasmalogin plasmalogin-fingerprint; do
    printf 'auth required pam_unix.so\naccount required pam_permit.so\nsession required pam_permit.so\n' > "$test_dir/pam/$service"
    cp "$test_dir/pam/$service" "$test_dir/$service.original"
done
# Include a shared Gaze entry: the managed KDE entry must still install its wallet handoff.
printf 'auth sufficient pam_gaze.so\n' > "$test_dir/pam/common-auth"
printf 'auth include common-auth\n' >> "$test_dir/pam/sddm"
cp "$test_dir/pam/sddm" "$test_dir/sddm.original"
sh "$helper" enable-login >/dev/null
sh "$helper" enable >/dev/null
for service in sddm plasmalogin plasmalogin-fingerprint; do
    test "$(grep -c 'pam_gaze.so kde-login' "$test_dir/pam/$service")" = 1
    test "$(grep -c 'session.*pam_kwallet5.so auto_start' "$test_dir/pam/$service")" = 1
    cp "$test_dir/pam/$service" "$test_dir/$service.managed"
done
! grep -q kwallet "$GAZE_KDE_PAM_FILE"
sh "$helper" status >/dev/null
sh "$helper" enable-login >/dev/null
for service in sddm plasmalogin plasmalogin-fingerprint; do
    cmp "$test_dir/pam/$service" "$test_dir/$service.managed"
done
sh "$helper" disable-login >/dev/null
for service in sddm plasmalogin plasmalogin-fingerprint; do
    cmp "$test_dir/pam/$service" "$test_dir/$service.original"
done
# Upgrade an older success=done managed block and keep the original distro lines.
begin='# BEGIN gaze (managed by gaze-kde; remove with `gaze-kde-pam disable`)'
{ printf '%s\nauth [success=done default=ignore] pam_gaze.so\n# END gaze\n' "$begin"; cat "$test_dir/sddm.original"; } > "$test_dir/pam/sddm"
sh "$helper" enable-login >/dev/null
grep -q 'pam_gaze.so kde-login' "$test_dir/pam/sddm"
sh "$helper" disable-login >/dev/null
cmp "$test_dir/pam/sddm" "$test_dir/sddm.original"
echo 'PASS: KDE install, repeat/upgrade, read-only status, removal, and lock-screen isolation.'

if [ "$(uname -s)" != Linux ] || ! command -v cc >/dev/null 2>&1; then
    echo 'SKIP: real KWallet PAM control-flow tests require Linux and a C compiler.'
    exit 0
fi
cc -Wall -Wextra -Werror -fPIC -shared -DGAZE_MOCK_MODULE "$repo/scripts/keyring-pam-harness.c" -lpam -o "$test_dir/mock.so"
cc -Wall -Wextra -Werror "$repo/scripts/keyring-pam-harness.c" -lpam -o "$test_dir/driver"
for service in sddm plasmalogin plasmalogin-fingerprint; do
    for result in 0 7 9 25; do
        for token in token empty; do
            for gate in pam_permit.so pam_deny.so; do
                marker="$test_dir/called"
                rm -f "$marker"
                # Keep the helper's real controls, replace implementations only. A denied
                # password fallback cannot turn a camera failure into successful auth.
                sed -e "s|pam_nologin.so|$gate|" \
                    -e 's/pam_faillock.so.*preauth/pam_permit.so/' \
                    -e "s|pam_gaze.so kde-login|$test_dir/mock.so gaze $result $token|" \
                    -e "s|pam_kwallet5.so auto_start|$test_dir/mock.so session $marker|" \
                    -e "s|pam_kwallet5.so|$test_dir/mock.so kwallet $marker $token|" \
                    -e 's/pam_unix.so/pam_deny.so/' \
                    -e '/auth include common-auth/d' \
                    "$test_dir/$service.managed" > "$test_dir/pam/$service"
                cp "$test_dir/pam/$service" "$test_dir/denied-password"
                for password in denied accepted; do
                    rm -f "$marker"
                    cp "$test_dir/denied-password" "$test_dir/pam/$service"
                    if [ "$password" = accepted ]; then
                        sed 's/auth required pam_deny.so/auth required pam_permit.so/' \
                            "$test_dir/denied-password" > "$test_dir/pam/$service"
                    fi
                    expected=failure
                    if [ "$gate" = pam_permit.so ] && { [ "$result" = 0 ] || [ "$password" = accepted ]; }; then
                        expected=success
                    fi
                    "$test_dir/driver" "$test_dir/pam" "$expected" "$service"
                    if [ "$gate" = pam_permit.so ] && [ "$result" = 0 ]; then
                        test "$(cat "$marker")" = valid-session
                    else
                        test ! -e "$marker"
                    fi
                done
            done
        done
    done
done
echo 'PASS: 96 KDE PAM cases; gated biometric success reaches wallet, with password fallback preserved.'

# Optional integration check against the actual built Rust module. No system bus is
# available in the test container, so the first entry fails quickly and sets the
# managed-attempt marker. Included sequential/simultaneous/retry entries must ignore it.
if [ -n "${GAZE_TEST_PAM_GAZE:-}" ]; then
    for service in sddm plasmalogin plasmalogin-fingerprint; do
        for mode in sequential simultaneous retry; do
            cat > "$test_dir/pam/$service" <<STACK
auth [default=ignore] $GAZE_TEST_PAM_GAZE kde-login
auth [ignore=1 default=die] $GAZE_TEST_PAM_GAZE $mode
auth requisite pam_deny.so
auth required pam_permit.so
session required pam_permit.so
STACK
            "$test_dir/driver" "$test_dir/pam" success "$service"
        done
    done
    echo 'PASS: real pam_gaze prevents duplicate scans in all three shared-stack modes.'
fi
