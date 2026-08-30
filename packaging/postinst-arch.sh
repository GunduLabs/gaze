#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

set -e

configure_pam_sudo() {
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

configure_pam_polkit() {
	pam_file=/etc/pam.d/polkit-1
	[ -f "$pam_file" ] && return 0

	cat > "$pam_file" <<-EOF
	#%PAM-1.0
	auth       sufficient   pam_gaze.so
	auth       include      system-auth
	account    include      system-auth
	password   include      system-auth
	session    include      system-auth
	EOF
	chmod 644 "$pam_file"

	mkdir -p /etc/gaze
	printf '%s\n' "$pam_file" > /etc/gaze/pam-arch.polkit-configured
}

check_deprecated_pam_grosshack() {
	if [ -d /etc/pam.d ] && grep -rnE '^[[:space:]]*[^#].*pam_gaze_grosshack\.so' /etc/pam.d/ >/dev/null 2>&1; then
		printf '\n\033[1;33m[Gaze Notice]\033[0m Found legacy pam_gaze_grosshack.so in /etc/pam.d/:\n' >&2
		grep -rnE '^[[:space:]]*[^#].*pam_gaze_grosshack\.so' /etc/pam.d/ 2>/dev/null | head -n 5 | sed 's/^/  /' >&2
		printf 'This module is deprecated and will be removed in a future release.\n' >&2
		printf 'Please update your PAM configuration to use: pam_gaze.so simultaneous\n' >&2
		printf 'Run "gaze doctor" for more information.\n\n' >&2
	fi
}

check_deprecated_pam_grosshack
configure_pam_sudo
configure_pam_polkit

if [ -d /run/systemd/system ]; then
	systemctl daemon-reload >/dev/null 2>&1 || true
	dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig >/dev/null 2>&1 || true
	systemctl restart polkit >/dev/null 2>&1 || true
	systemctl try-restart gazed >/dev/null 2>&1 || true
fi
