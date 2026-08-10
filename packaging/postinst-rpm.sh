#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

set -e

# Regenerate PAM files after a Gaze profile update, but only when Gaze is
# already selected. Never replace another active authselect profile.
if command -v authselect >/dev/null 2>&1 &&
	authselect current --raw 2>/dev/null | grep -q '^gaze\([[:space:]]\|$\)'; then
	authselect apply-changes >/dev/null 2>&1 || true
fi

# A display-manager greeter runs as xdm_t, which SELinux denies the camera by
# default; the denial is silent and reads like a broken camera. Loaded here
# because the base package owns the policy, whichever desktop is installed.
if [ -f /usr/share/gaze/gaze-gdm-camera.pp ] && command -v semodule >/dev/null 2>&1; then
	semodule -i /usr/share/gaze/gaze-gdm-camera.pp >/dev/null 2>&1 || true
fi

if [ -d /run/systemd/system ]; then
	systemctl daemon-reload >/dev/null 2>&1 || true
	dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig >/dev/null 2>&1 || true
	systemctl restart polkit >/dev/null 2>&1 || true
	systemctl try-restart gazed >/dev/null 2>&1 || true
fi
