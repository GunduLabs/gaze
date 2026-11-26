#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e

case "${1:-}" in
1 | 2 | upgrade | failed-upgrade | abort-upgrade | abort-install) exit 0 ;;
esac

[ -x /usr/bin/gaze-kde-pam ] || exit 0
/usr/bin/gaze-kde-pam disable || true
/usr/bin/gaze-kde-pam disable-login || true
