#!/bin/sh
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

set -e

# RPM passes 0 for erase and 1 for upgrade.
# Remove modules only while vendor definitions still exist.
[ "${1:-}" = 0 ] || exit 0

[ -r /etc/os-release ] || exit 0
# shellcheck disable=SC1091
. /etc/os-release
case "${ID:-} ${ID_LIKE:-}" in
	*opensuse*|*suse*) ;;
	*) exit 0 ;;
esac

command -v pam-config >/dev/null 2>&1 || exit 0
pam-config -d --gaze >/dev/null 2>&1 || true
pam-config -d --gaze_grosshack >/dev/null 2>&1 || true
