#!/bin/sh
set -e

if [ -d /run/systemd/system ]; then
	systemctl daemon-reload >/dev/null 2>&1
	dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig >/dev/null 2>&1 || true
	systemctl restart polkit >/dev/null 2>&1 || true
	systemctl enable --now gazed >/dev/null 2>&1 || true
	pam-auth-update --package >/dev/null 2>&1 || true
fi
