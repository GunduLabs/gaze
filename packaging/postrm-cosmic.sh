#!/bin/sh
set -e

case "${1:-}" in
1 | 2 | upgrade | failed-upgrade | abort-upgrade | abort-install) exit 0 ;;
esac

pam_file=/etc/pam.d/cosmic-greeter
begin_marker='# BEGIN gaze (managed by gaze-cosmic; remove with `gaze-cosmic-pam disable`)'
end_marker='# END gaze'

if [ -f "$pam_file" ] && grep -qF "$begin_marker" "$pam_file" 2>/dev/null; then
	tmp=$(mktemp)
	awk -v begin="$begin_marker" -v end="$end_marker" '
		$0 == begin && !holding { holding = 1; held = ""; next }
		holding && $0 == end { holding = 0; next }
		holding { held = held $0 "\n"; next }
		{ print }
		END { if (holding) printf "%s\n%s", begin, held }
	' "$pam_file" >"$tmp"

	if cmp -s "$pam_file" "$tmp"; then
		rm -f "$tmp"
	else
		staged="$pam_file.gaze-staged.$$"
		cp -a "$pam_file" "$staged"
		cat "$tmp" >"$staged"
		sync "$staged" 2>/dev/null || true
		mv -f "$staged" "$pam_file"
		rm -f "$tmp"
		if command -v restorecon >/dev/null 2>&1; then
			restorecon "$pam_file" >/dev/null 2>&1 || true
		fi

		cat <<'EOF'

Gaze face authentication removed from /etc/pam.d/cosmic-greeter.
The COSMIC lock and login screens are back to password-only authentication.
EOF
	fi
fi

rm -f /etc/gaze/cosmic-greeter.pam.orig 2>/dev/null || true
