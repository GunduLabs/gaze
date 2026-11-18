#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Repack ONNX Runtime from Microsoft's NuGet package into DEST, laid out like the upstream
# release tarball. The library is loaded by a root daemon, so only a pinned SHA256 is accepted.
set -euo pipefail
version=${1:?ONNX Runtime version is required}
ort_arch=${2:?ONNX Runtime architecture (x64 or aarch64) is required}
dest=${3:?destination path is required}
case "$ort_arch" in
    x64) rid=linux-x64 ;;
    aarch64) rid=linux-arm64 ;;
    *) echo "Unsupported ONNX Runtime architecture: $ort_arch" >&2; exit 1 ;;
esac
case "$version" in
    1.32.0) nupkg_sha256=a9150bc2992cbbb6722070da9910e1ffa989a8bfc67bdd5bb8e745ca755641db ;;
    *) nupkg_sha256=${ORT_NUPKG_SHA256:-} ;;
esac
case "$version-$ort_arch" in
    1.32.0-x64) lib_sha256=4213d6d8c42ac4754865849cb139d31fc39127db6a326babf90bbc81a6eb52fc ;;
    1.32.0-aarch64) lib_sha256=fd1db07c49cb1ad11370ef4393a405eef959fa9622fb5b2880f85d025757e719 ;;
    *) lib_sha256= ;;
esac
if [ -z "$nupkg_sha256" ]; then
    echo "No pinned SHA256 for the ONNX Runtime $version NuGet package; add it to $0 or set ORT_NUPKG_SHA256" >&2
    exit 1
fi
name="onnxruntime-linux-$ort_arch-$version"
lib="libonnxruntime.so.$version"
lib_matches() { [ -n "$lib_sha256" ] && [ "$(sha256sum | cut -d' ' -f1)" = "$lib_sha256" ]; }
if [ -f "$dest" ] && tar -xzOf "$dest" "$name/lib/$lib" 2>/dev/null | lib_matches; then
    exit 0
fi
mkdir -p "$(dirname "$dest")"
work=$(mktemp -d)
archive=$(mktemp "$dest.XXXXXX")
trap 'rm -rf "$work" "$archive"' EXIT
curl --fail --location --retry 3 \
    "https://api.nuget.org/v3-flatcontainer/microsoft.ml.onnxruntime/$version/microsoft.ml.onnxruntime.$version.nupkg" \
    -o "$work/ort.nupkg"
if ! echo "$nupkg_sha256  $work/ort.nupkg" | sha256sum --check --status; then
    echo "ONNX Runtime $version NuGet package does not match its pinned SHA256" >&2
    exit 1
fi
mkdir -p "$work/$name/lib"
unzip -qj "$work/ort.nupkg" LICENSE ThirdPartyNotices.txt -d "$work/$name"
unzip -qj "$work/ort.nupkg" "runtimes/$rid/native/libonnxruntime.so" -d "$work/$name/lib"
mv "$work/$name/lib/libonnxruntime.so" "$work/$name/lib/$lib"
if [ -n "$lib_sha256" ] && ! lib_matches < "$work/$name/lib/$lib"; then
    echo "ONNX Runtime $version ($ort_arch) library does not match its pinned SHA256" >&2
    exit 1
fi
chmod 0644 "$work/$name/LICENSE" "$work/$name/ThirdPartyNotices.txt" "$work/$name/lib/$lib"
ln -s "$lib" "$work/$name/lib/libonnxruntime.so.1"
ln -s libonnxruntime.so.1 "$work/$name/lib/libonnxruntime.so"
tar --owner=0 --group=0 --numeric-owner -czf "$archive" -C "$work" "$name"
mv "$archive" "$dest"
