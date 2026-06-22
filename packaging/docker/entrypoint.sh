#!/usr/bin/env bash
# Entrypoint for the Gaze Linux build container. Receives a just target plus args,
# e.g. `build-rust`, `build-flatpak`, `package-prebuilt deb`.
set -euo pipefail

# Flatpak builds need the flathub remote and the GNOME/Rust/LLVM SDKs (cd.yml
# installs these as separate steps). Do it lazily, only when a flatpak target is
# requested. The SDKs land in the gaze-flatpak volume, so this is a one-time
# download that subsequent runs skip.
if printf '%s ' "$@" | grep -q 'flatpak'; then
    flatpak remote-add --user --if-not-exists flathub \
        https://flathub.org/repo/flathub.flatpakrepo
    # Keep these branch versions in sync with .github/workflows/cd.yml.
    flatpak install --user -y flathub \
        org.gnome.Sdk//49 \
        org.gnome.Platform//49 \
        org.freedesktop.Sdk.Extension.rust-stable//25.08 \
        org.freedesktop.Sdk.Extension.llvm20//25.08
fi

rc=0
just "$@" || rc=$?

# Hand any container-created artifacts back to the host user. Harmless no-op on
# Docker Desktop (which already maps ownership to the host user).
if [ -n "${HOST_UID:-}" ] && [ -n "${HOST_GID:-}" ]; then
    chown -R "${HOST_UID}:${HOST_GID}" \
        dist .flatpak-cache vendor flatpak-build .flatpak-builder 2>/dev/null || true
fi

exit "$rc"
