#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

set -e

case "${1:-}" in
	remove|purge)
		pam-auth-update --package --remove gaze >/dev/null 2>&1 || true
		pam-auth-update --package --remove gaze-simultaneous >/dev/null 2>&1 || true
		;;
esac
