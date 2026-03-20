#!/bin/sh
set -e

if [ -d /run/systemd/system ]; then
    glib-compile-schemas /usr/share/glib-2.0/schemas >/dev/null 2>&1

    if command -v semodule >/dev/null 2>&1; then
        semodule -i /usr/share/gaze/gaze-gdm-camera.pp >/dev/null 2>&1 || true
    fi
fi
