#!/bin/sh
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
KEY_URL="${PKG_BASE_URL}/PACKAGE-SIGNING-KEY.asc"

if command -v curl >/dev/null 2>&1; then
	curl -fsSL "$KEY_URL" -o /tmp/gundulabs-packages.asc || true
elif command -v wget >/dev/null 2>&1; then
	wget -qO /tmp/gundulabs-packages.asc "$KEY_URL" || true
fi

if [ -f /tmp/gundulabs-packages.asc ]; then
	FPR="$(gpg --show-keys --with-colons /tmp/gundulabs-packages.asc | awk -F: '/^fpr:/ {print $10; exit}')"
	if [ -n "$FPR" ]; then
		pacman-key --add /tmp/gundulabs-packages.asc || true
		pacman-key --lsign-key "$FPR" || true
	fi
	rm -f /tmp/gundulabs-packages.asc
fi

cat >/etc/pacman.d/gaze-mirrorlist <<EOF
Server = ${PKG_BASE_URL}/arch/x86_64
EOF

if ! grep -Eq '^\[gaze\]$' /etc/pacman.conf; then
	cat >>/etc/pacman.conf <<'EOF'

[gaze]
SigLevel = Required DatabaseOptional
Include = /etc/pacman.d/gaze-mirrorlist
EOF
fi

mkdir -p /var/lib/gaze/users
mkdir -p /opt/gaze/models
systemctl daemon-reload
dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig &> /dev/null || true