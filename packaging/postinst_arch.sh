#!/bin/sh
set -e
mkdir -p /var/lib/gaze/users
mkdir -p /opt/gaze/models
systemctl daemon-reload
dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig 2>/dev/null || true
