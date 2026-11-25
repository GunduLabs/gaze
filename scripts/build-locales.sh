#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

set -euo pipefail

[ $# -eq 2 ] || { echo "usage: $0 <po-dir> <locale-dir>" >&2; exit 2; }
command -v msgfmt >/dev/null 2>&1 || { echo "$0: msgfmt (gettext) is required" >&2; exit 1; }

for po in "$1"/*.po; do
    lang=$(basename "$po" .po)
    dir="$2/$lang/LC_MESSAGES"
    install -d -m 0755 "$dir"
    msgfmt --check-format --check-domain -o "$dir/gaze@gundulabs.com.mo" "$po"
    chmod 0644 "$dir/gaze@gundulabs.com.mo"
done
