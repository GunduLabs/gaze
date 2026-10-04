#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Stage the CPU runtime beside the daemon, including for test executables in deps/.
# Packaging installs this same file into /usr/lib/gaze, without a global ldconfig entry.
set -euo pipefail
version=${1:?ONNX Runtime version is required}
arch=${2:?target architecture is required}
case "$arch" in
    x86_64) ort_arch=x64 ;;
    aarch64) ort_arch=aarch64 ;;
    *) echo "Unsupported Linux runtime architecture: $arch" >&2; exit 1 ;;
esac
cache="${ORT_CACHE_DIR:-target/ort-native}/$version/$arch"
archive="$cache/onnxruntime.tgz"
stamp="$cache/.extracted"
"$(dirname "$0")/fetch-ort.sh" "$version" "$ort_arch" "$archive"
if [ ! -f "$stamp" ] || [ "$archive" -nt "$stamp" ]; then
    rm -rf "$cache/lib"
    tar --no-same-owner -xzf "$archive" -C "$cache" --strip-components=1
    touch "$stamp"
fi
mkdir -p target/release
cp -L "$cache/lib/libonnxruntime.so" target/release/libonnxruntime.so
cp "$cache/LICENSE" target/release/onnxruntime-LICENSE
cp "$cache/ThirdPartyNotices.txt" target/release/onnxruntime-NOTICES
