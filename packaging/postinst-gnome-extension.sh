#!/bin/sh
set -e

if [ -d /run/systemd/system ]; then
    dconf update >/dev/null 2>&1 || true
    glib-compile-schemas /usr/share/glib-2.0/schemas >/dev/null 2>&1 || true

    if command -v semodule >/dev/null 2>&1; then
        semodule -i /usr/share/gaze/gaze-gdm-camera.pp >/dev/null 2>&1 || true
    fi
fi
