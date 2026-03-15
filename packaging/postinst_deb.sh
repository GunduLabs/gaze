#!/bin/sh
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
KEY_URL="${PKG_BASE_URL}/PACKAGE-SIGNING-KEY.asc"

fetch_key() {
	if command -v curl >/dev/null 2>&1; then
		curl -fsSL "$KEY_URL"
		return 0
	fi
	if command -v wget >/dev/null 2>&1; then
		wget -qO- "$KEY_URL"
		return 0
	fi
	return 1
}

install -d -m 0755 /usr/share/keyrings
if fetch_key >/tmp/gundulabs-packages.asc; then
	if command -v gpg >/dev/null 2>&1; then
		gpg --dearmor --yes -o /usr/share/keyrings/gundulabs-packages.gpg /tmp/gundulabs-packages.asc
	else
		cp /tmp/gundulabs-packages.asc /usr/share/keyrings/gundulabs-packages.gpg
	fi
	cat >/etc/apt/sources.list.d/gaze.list <<EOF
deb [signed-by=/usr/share/keyrings/gundulabs-packages.gpg] ${PKG_BASE_URL}/deb stable main
EOF
fi
rm -f /tmp/gundulabs-packages.asc

mkdir -p /var/lib/gaze/users
mkdir -p /opt/gaze/models
systemctl daemon-reload
dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig &> /dev/null || true
systemctl enable --now gazed
pam-auth-update --package