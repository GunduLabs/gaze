#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e

# Clean up only on real removal, not on upgrade.
case "${1:-}" in
1 | 2 | upgrade | failed-upgrade | abort-upgrade | abort-install) exit 0 ;;
esac

lock_pam_files="/etc/pam.d/kde-fingerprint /etc/pam.d/kde-smartcard"
login_pam_files="/etc/pam.d/plasmalogin /etc/pam.d/sddm"
login_face_pam_file=/etc/pam.d/plasmalogin-fingerprint
vendor_pam_dir=/usr/lib/pam.d
state_dir=/etc/gaze
login_flag="$state_dir/login-enabled"
begin_marker='# BEGIN gaze (managed by gaze-kde; remove with `gaze-kde-pam disable`)'
end_marker='# END gaze'

strip_gaze_block() {
	target=$1
	[ -f "$target" ] || return 0
	grep -qF "$begin_marker" "$target" 2>/dev/null || return 0

	tmp=$(mktemp)
	awk -v begin="$begin_marker" -v end="$end_marker" '
		$0 == begin && !holding { holding = 1; held = ""; next }
		holding && $0 == end { holding = 0; next }
		holding { held = held $0 "\n"; next }
		{ print }
		END { if (holding) printf "%s\n%s", begin, held }
	' "$target" >"$tmp"

	if cmp -s "$target" "$tmp"; then
		rm -f "$tmp"
		return 0
	fi

	staged="$target.gaze-staged.$$"
	cp -a "$target" "$staged"
	cat "$tmp" >"$staged"
	sync "$staged" 2>/dev/null || true
	mv -f "$staged" "$target"
	rm -f "$tmp"
	if command -v restorecon >/dev/null 2>&1; then
		restorecon "$target" >/dev/null 2>&1 || true
	fi
}

# Files Gaze created itself go away whole, restoring the vendor stack underneath
# where there is one. The recorded checksum is what Gaze left behind, so a file
# someone has edited since is kept, while a copy the vendor has changed underneath
# is still removed rather than shadowing the new vendor stack for good.
file_sum() {
	[ -f "$1" ] || return 1
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum <"$1" | cut -d' ' -f1
	else
		cksum <"$1" | cut -d' ' -f1,2
	fi
}

# Decided before the block is stripped, since stripping is itself a change.
untouched=""
for target in $lock_pam_files $login_face_pam_file; do
	base=$(basename "$target")
	[ -f "$state_dir/$base.created-by-gaze" ] || continue
	recorded=$(cat "$state_dir/$base.installed-sha256" 2>/dev/null) || recorded=
	if [ -n "$recorded" ] && [ "$recorded" = "$(file_sum "$target" 2>/dev/null)" ]; then
		untouched="$untouched $target"
	fi
done

for target in $lock_pam_files $login_pam_files $login_face_pam_file; do
	strip_gaze_block "$target"
done

for target in $lock_pam_files $login_face_pam_file; do
	base=$(basename "$target")
	case " $untouched " in
	*" $target "*) rm -f "$target" ;;
	esac
	rm -f "$state_dir/$base.created-by-gaze" "$state_dir/$base.installed-sha256" \
		"$state_dir/$base.vendor-sha256"
done
rm -f "$login_flag" "$state_dir/lock-disabled"

cat <<'EOF'

Gaze face unlock removed from the KDE Plasma lock screen and login greeter.
Both are back to password-only authentication.
EOF
