#!/bin/sh
set -e

configure_pam() {
	pam_file=/etc/pam.d/sudo
	[ -f "$pam_file" ] || return 0
	grep -q "pam_gaze" "$pam_file" 2>/dev/null && return 0

	tmp=$(mktemp)
	awk '
		/^[[:space:]]*auth[[:space:]]/ && !done {
			print "auth        sufficient    pam_gaze.so"
			done = 1
		}
		{ print }
	' "$pam_file" > "$tmp" && install -m 644 "$tmp" "$pam_file"
	rm -f "$tmp"

	mkdir -p /etc/gaze
	printf '%s\n' "$pam_file" > /etc/gaze/pam-arch.configured
}

configure_pam

if [ -d /run/systemd/system ]; then
	systemctl daemon-reload >/dev/null 2>&1
	dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig >/dev/null 2>&1 || true
	systemctl restart polkit >/dev/null 2>&1 || true
fi
