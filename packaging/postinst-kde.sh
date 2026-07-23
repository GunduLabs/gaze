#!/bin/sh
set -e

PAM_FILE=/etc/pam.d/kde-fingerprint
REFERENCE=/usr/share/gaze/pam/kde-fingerprint
GAZE_LINE='auth        [success=done default=ignore]                pam_gaze.so'

pam_gaze_active() {
	grep -Eq '^[[:space:]]*auth[[:space:]].*pam_gaze\.so([[:space:]]|$)' "$1"
}

if [ -f "$PAM_FILE" ]; then
	if ! pam_gaze_active "$PAM_FILE"; then
		tmp=$(mktemp)
		awk -v line="$GAZE_LINE" '
			!inserted && $0 ~ /^[[:space:]]*auth[[:space:]]/ { print line; inserted = 1 }
			{ print }
			END { if (!inserted) print line }
		' "$PAM_FILE" >"$tmp"
		cat "$tmp" >"$PAM_FILE"
		rm -f "$tmp"
	fi
elif [ -f "$REFERENCE" ]; then
	cp "$REFERENCE" "$PAM_FILE"
else
	printf '%s\n' '#%PAM-1.0' "$GAZE_LINE" >"$PAM_FILE"
fi

cat <<'EOF'

Gaze KDE integration enabled in /etc/pam.d/kde-fingerprint.
Face authentication now runs on the KDE Plasma lock screen and login
greeter, in parallel with the password field, so a face match unlocks
without pressing a key. Lock your screen and look at the camera.

Docs: https://gaze.gundulabs.com/guide/kde
EOF
