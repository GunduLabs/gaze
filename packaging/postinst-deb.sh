#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

set -e

cpu_lacks_avx2() {
	case "$(uname -m)" in
		x86_64) ;;
		*) return 1 ;;
	esac
	! grep -qw avx2 /proc/cpuinfo 2>/dev/null
}

warn_missing_avx2() {
	cpu_lacks_avx2 || return 0
	printf '\n\033[1;33m[Gaze Notice]\033[0m This CPU does not support AVX2.\n' >&2
	printf 'The gazed daemon cannot run here: it would crash with an illegal instruction\n' >&2
	printf 'on every start. The CLI and PAM modules are installed, but face authentication\n' >&2
	printf 'will not work. gazed has been left stopped.\n' >&2
	printf 'See https://gaze.gundulabs.com/guide/troubleshooting\n\n' >&2
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
warn_missing_avx2

if [ -d /run/systemd/system ]; then
	systemctl daemon-reload >/dev/null 2>&1 || true
	dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig >/dev/null 2>&1 || true
	systemctl restart polkit >/dev/null 2>&1 || true
	# Starting gazed on a CPU without AVX2 only produces a SIGILL restart loop.
	if ! cpu_lacks_avx2; then
		systemctl enable --now gazed >/dev/null 2>&1 || true
		systemctl try-restart gazed >/dev/null 2>&1 || true
	fi
	pam-auth-update --package >/dev/null 2>&1 || true
fi
