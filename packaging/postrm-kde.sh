#!/bin/sh
set -e

PAM_FILE=/etc/pam.d/kde-fingerprint

# Skip on upgrade (deb "upgrade", rpm $1 >= 1); clean up only on real removal
# (rpm "0", deb "remove"/"purge", arch runs this only on removal).
case "${1:-0}" in
	upgrade) exit 0 ;;
	0 | remove | purge) ;;
	*[!0-9]*) ;;
	*) exit 0 ;;
esac

if [ -f "$PAM_FILE" ]; then
	tmp=$(mktemp)
	grep -Ev '^[[:space:]]*auth[[:space:]].*pam_gaze\.so([[:space:]]|$)' "$PAM_FILE" >"$tmp" || true
	cat "$tmp" >"$PAM_FILE"
	rm -f "$tmp"
fi

cat <<'EOF'

Gaze KDE integration removed. The KDE lock screen and greeter fall back to
their default password authentication.
EOF
