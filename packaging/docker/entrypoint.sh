#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Entrypoint for the Gaze Linux build container. Receives a just target plus args,
# e.g. `build-rust`, `build-flatpak`, `package-prebuilt deb`.
set -euo pipefail

# Git rejects the host-owned /work bind mount as "dubious ownership" when run as root here,
# and packaging recipes need `git describe` for the version, so trust it before running just.
git config --global --add safe.directory /work

rc=0
just "$@" || rc=$?

# Re-home root-written artifacts to the host user, but skip files it already owns. Under a
# uid-mapping backend (Colima's sshfs) chowning those pins a uid the next run cannot write.
if [ -n "${HOST_UID:-}" ] && [ -n "${HOST_GID:-}" ]; then
    for d in dist .flatpak-cache vendor; do
        [ -e "$d" ] || continue
        find "$d" \! -uid "${HOST_UID}" -exec chown "${HOST_UID}:${HOST_GID}" {} + 2>/dev/null || true
    done
fi

exit "$rc"
