#!/usr/bin/env bash
set -euo pipefail

MANIFEST="${1:-packaging/flatpak/com.gundulabs.Gaze.yml}"
BUILD_DIR="${2:-.flatpak-builder}"
REPO_DIR="${3:-dist/repos/flatpak}"
BUNDLE_PATH="${4:-dist/packages/com.gundulabs.Gaze.flatpak}"
APP_ID="com.gundulabs.Gaze"
ARCH="$(flatpak --default-arch)"

mkdir -p "$(dirname "$BUNDLE_PATH")"
mkdir -p "$REPO_DIR"

flatpak-builder \
  --force-clean \
  --repo="$REPO_DIR" \
  --arch="$ARCH" \
  --install-deps-from=flathub \
  --user \
  --share=network \
  "$BUILD_DIR" \
  "$MANIFEST"

flatpak build-bundle \
  "$REPO_DIR" \
  "$BUNDLE_PATH" \
  "$APP_ID"
