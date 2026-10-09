#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Stage Intel's onnxruntime-openvino runtime in DEST for `just test-openvino`, accepting
# only a pinned SHA256. Intel ships it only as a Python wheel; no package installs it.
set -euo pipefail
version=${1:?onnxruntime-openvino version is required}
dest=${2:?destination directory is required}
case "$version" in
    1.24.1) sha256=d617fac2f59a6ab5ea59a788c3e1592240a129642519aaeaa774761dfe35150e ;;
    *) sha256=${ORT_OPENVINO_SHA256:-} ;;
esac
if [ -z "$sha256" ]; then
    echo "No pinned SHA256 for onnxruntime-openvino $version; add it to $0 or set ORT_OPENVINO_SHA256" >&2
    exit 1
fi
wheel="onnxruntime_openvino-$version-cp312-cp312-manylinux_2_28_x86_64.whl"
cache="${ORT_CACHE_DIR:-target/ort-native}/openvino/$version"
verified() { echo "$sha256  $1" | sha256sum --check --status; }
if [ ! -f "$cache/$wheel" ] || ! verified "$cache/$wheel"; then
    download=$(mktemp -d "${TMPDIR:-/tmp}/ort-openvino.XXXXXX")
    trap 'rm -rf "$download"' EXIT
    python3 -m pip download --quiet --no-deps --only-binary :all: \
        --python-version 3.12 --platform manylinux_2_28_x86_64 \
        "onnxruntime-openvino==$version" -d "$download"
    if ! verified "$download/$wheel"; then
        echo "onnxruntime-openvino $version does not match its pinned SHA256" >&2
        exit 1
    fi
    mkdir -p "$cache"
    mv "$download/$wheel" "$cache/$wheel"
fi
stamp="$dest/.extracted-$version"
if [ ! -f "$stamp" ] || [ "$cache/$wheel" -nt "$stamp" ]; then
    rm -rf "${dest:?}"
    mkdir -p "$dest"
    unzip -qoj "$cache/$wheel" 'onnxruntime/capi/lib*.so*' -d "$dest"
    real="libonnxruntime.so.$version"
    soname=$(objdump -p "$dest/$real" | awk '/SONAME/ { print $2 }')
    test -n "$soname"
    ln -sf "$real" "$dest/libonnxruntime.so"
    ln -sf "$real" "$dest/$soname"
    if ! objdump -T "$dest/$real" | grep -q OrtSessionOptionsAppendExecutionProvider_OpenVINO; then
        echo "onnxruntime-openvino $version does not export the OpenVINO execution provider" >&2
        exit 1
    fi
    touch "$stamp"
fi
