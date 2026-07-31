#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e

# Clean up only on real removal, not on upgrade.
case "${1:-}" in
1 | 2 | upgrade | failed-upgrade | abort-upgrade | abort-install) exit 0 ;;
esac

pam_file=/etc/pam.d/kde-fingerprint
login_pam_files="/etc/pam.d/plasmalogin /etc/pam.d/sddm"
created_flag=/etc/gaze/kde-fingerprint.created-by-gaze
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

strip_gaze_block "$pam_file"
for target in $login_pam_files; do
	strip_gaze_block "$target"
done

if [ -f "$created_flag" ]; then
	rm -f "$pam_file" "$created_flag"
fi

cat <<'EOF'

Gaze face unlock removed from the KDE Plasma lock screen and login greeter.
Both are back to password-only authentication.
EOF
