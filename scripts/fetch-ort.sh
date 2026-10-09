#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Download an ONNX Runtime release archive to DEST, accepting only a pinned SHA256.
# The library is loaded by a root daemon, so an unverified archive is never kept.
set -euo pipefail
version=${1:?ONNX Runtime version is required}
ort_arch=${2:?ONNX Runtime architecture (x64 or aarch64) is required}
dest=${3:?destination path is required}
case "$version-$ort_arch" in
    1.31.0-x64) sha256=cc5c72baf5ae5c8238a6841f0897227be2d02826b9cf98eaf02fdefaeeb03a57 ;;
    1.31.0-aarch64) sha256=c5b8b3cca31f3d643a3b313b8f42e2d0f16a4fcefd02cda343d97bd5306afaab ;;
    *) sha256=${ORT_SHA256:-} ;;
esac
if [ -z "$sha256" ]; then
    echo "No pinned SHA256 for ONNX Runtime $version ($ort_arch); add it to $0 or set ORT_SHA256" >&2
    exit 1
fi
verified() { echo "$sha256  $1" | sha256sum --check --status; }
if [ -f "$dest" ] && verified "$dest"; then
    exit 0
fi
mkdir -p "$(dirname "$dest")"
download=$(mktemp "$dest.XXXXXX")
trap 'rm -f "$download"' EXIT
curl --fail --location --retry 3 \
    "https://github.com/microsoft/onnxruntime/releases/download/v$version/onnxruntime-linux-$ort_arch-$version.tgz" \
    -o "$download"
if ! verified "$download"; then
    echo "ONNX Runtime $version ($ort_arch) does not match its pinned SHA256" >&2
    exit 1
fi
mv "$download" "$dest"
