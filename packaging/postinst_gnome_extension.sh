#!/bin/sh
set -e

glib-compile-schemas /usr/share/glib-2.0/schemas

POLICY_DIR="/usr/share/gaze"
MODULE="gaze-gdm-camera"

if command -v checkmodule >/dev/null 2>&1 && command -v semodule_package >/dev/null 2>&1; then
    checkmodule -M -m -o "${POLICY_DIR}/${MODULE}.mod" "${POLICY_DIR}/${MODULE}.te"
    semodule_package -o "${POLICY_DIR}/${MODULE}.pp" -m "${POLICY_DIR}/${MODULE}.mod"
    semodule -i "${POLICY_DIR}/${MODULE}.pp"
    rm -f "${POLICY_DIR}/${MODULE}.mod" "${POLICY_DIR}/${MODULE}.pp"
fi
