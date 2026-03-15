#!/bin/sh
set -e

PKG_BASE_URL="https://packages.gundulabs.com"
KEY_URL="${PKG_BASE_URL}/PACKAGE-SIGNING-KEY.asc"

rpm --import "$KEY_URL" || true
cat >/etc/yum.repos.d/gaze.repo <<EOF
[gaze]
name=Gundu Labs Packages
baseurl=${PKG_BASE_URL}/rpm/x86_64
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=${KEY_URL}
EOF

mkdir -p /var/lib/gaze/users
mkdir -p /opt/gaze/models
systemctl daemon-reload
dbus-send --system --type=method_call --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ReloadConfig &> /dev/null || true
rm -rf /etc/authselect/custom/gaze 2>/dev/null || true