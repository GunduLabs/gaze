#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later
set -e

case "${1:-}" in
1 | 2 | upgrade | failed-upgrade | abort-upgrade | abort-install) exit 0 ;;
esac

rm -f /etc/gaze/lock-disabled /etc/gaze/login-enabled
