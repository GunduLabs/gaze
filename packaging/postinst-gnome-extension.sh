#!/bin/sh
set -e

if [ -d /run/systemd/system ]; then
    glib-compile-schemas /usr/share/glib-2.0/schemas >/dev/null 2>&1

    # Update dconf database so GDM picks up the gaze schema defaults
    if command -v dconf >/dev/null 2>&1; then
        dconf update >/dev/null 2>&1 || true
    fi

    # Enable the GNOME Shell extension for GDM
    if command -v gnome-extensions >/dev/null 2>&1; then
        gnome-extensions enable gaze@gundulabs.com >/dev/null 2>&1 || true
    fi

    if command -v semodule >/dev/null 2>&1; then
        semodule -i /usr/share/gaze/gaze-gdm-camera.pp >/dev/null 2>&1 || true
    fi
fi
