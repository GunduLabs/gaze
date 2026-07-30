#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e

case "${1:-}" in
    1|2|upgrade|failed-upgrade) ;;
    *) rm -f /etc/dconf/db/gdm.d/99-gaze* 2>/dev/null || true ;;
esac

if [ -d /run/systemd/system ]; then
    dconf update >/dev/null 2>&1 || true
    glib-compile-schemas /usr/share/glib-2.0/schemas >/dev/null 2>&1 || true
fi
