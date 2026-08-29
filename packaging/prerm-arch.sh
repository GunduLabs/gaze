#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

set -e

flag=/etc/gaze/pam-arch.configured
if [ -f "$flag" ]; then
    while IFS= read -r f; do
        [ -f "$f" ] || continue
        sed -i '/pam_gaze/d' "$f" || true
    done < "$flag"
    rm -f "$flag" || true
fi

sed -i '/pam_gaze/d' /etc/pam.d/sudo 2>/dev/null || true

flag=/etc/gaze/pam-arch.polkit-configured
if [ -f "$flag" ]; then
    while IFS= read -r f; do
        rm -f "$f" || true
    done < "$flag"
    rm -f "$flag" || true
fi

if [ -d /run/systemd/system ]; then
    systemctl restart polkit >/dev/null 2>&1 || true
fi
