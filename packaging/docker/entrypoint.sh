#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Entrypoint for the Gaze Linux build container. Receives a just target plus args,
# e.g. `build-rust`, `build-flatpak`, `package-prebuilt deb`.
set -euo pipefail

# /work is bind-mounted from the host and owned by the host user, so git (run as
# root here) rejects it as "dubious ownership". Packaging recipes call
# `git describe` to derive the version, so trust the mount before running just.
git config --global --add safe.directory /work

rc=0
just "$@" || rc=$?

# Hand container-created artifacts back to the host user, but ONLY the ones that
# aren't already owned by it. On native Docker the container writes as root, so
# this re-homes them. On a uid-mapping backend (Colima's sshfs) the files already
# belong to the host user; chowning them there pins them to the literal host uid,
# after which the next run's container user can no longer write, so skip those.
if [ -n "${HOST_UID:-}" ] && [ -n "${HOST_GID:-}" ]; then
    for d in dist .flatpak-cache vendor; do
        [ -e "$d" ] || continue
        find "$d" \! -uid "${HOST_UID}" -exec chown "${HOST_UID}:${HOST_GID}" {} + 2>/dev/null || true
    done
fi

exit "$rc"
