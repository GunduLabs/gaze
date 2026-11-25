#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

# Optional backends stay off: each one would add back a shared library dependency.
set -euo pipefail
version=${1:?OpenCV version is required}
dest=${2:?destination path is required}
case "$version" in
    4.14.0) sha256=ee8fb9b30eb60850431b4656447080e3737b56e45719c92b67f245950609f86e ;;
    *) sha256=${OPENCV_SHA256:-} ;;
esac
if [ -z "$sha256" ]; then
    echo "No pinned SHA256 for OpenCV $version; add it to $0 or set OPENCV_SHA256" >&2
    exit 1
fi
stamp="$dest/.built"
[ ! -f "$stamp" ] || exit 0
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl --proto '=https' --tlsv1.2 --fail --location --retry 3 \
    "https://github.com/opencv/opencv/archive/refs/tags/$version.tar.gz" -o "$work/opencv.tar.gz"
if ! echo "$sha256  $work/opencv.tar.gz" | sha256sum --check --status; then
    echo "OpenCV $version source does not match its pinned SHA256" >&2
    exit 1
fi
mkdir "$work/src"
tar --no-same-owner -xzf "$work/opencv.tar.gz" -C "$work/src" --strip-components=1

flags=(
    -DCMAKE_BUILD_TYPE=Release
    -DCMAKE_INSTALL_PREFIX="$dest"
    -DCMAKE_INSTALL_LIBDIR=lib
    "-DBUILD_LIST=core,imgproc"
    -DBUILD_SHARED_LIBS=OFF
    -DENABLE_PIC=ON
    -DPARALLEL_ENABLE_PLUGINS=OFF
)
for option in BUILD_EXAMPLES BUILD_JAVA BUILD_PERF_TESTS BUILD_TESTS BUILD_ZLIB BUILD_opencv_apps \
    BUILD_opencv_python3 BUILD_ITT CV_TRACE WITH_1394 WITH_AVIF WITH_CAROTENE WITH_EIGEN WITH_FFMPEG \
    WITH_FLATBUFFERS WITH_GSTREAMER WITH_GTK WITH_IPP WITH_ITT WITH_JASPER WITH_JPEG WITH_KLEIDICV \
    WITH_LAPACK WITH_OBSENSOR WITH_OPENCL WITH_OPENEXR WITH_OPENJPEG WITH_OPENMP WITH_PNG WITH_PROTOBUF \
    WITH_QUIRC WITH_TBB WITH_TIFF WITH_V4L WITH_VA WITH_VA_INTEL WITH_VTK WITH_WEBP; do
    flags+=("-D$option=OFF")
done
rm -rf "$dest"
cmake -S "$work/src" -B "$work/build" "${flags[@]}"
cmake --build "$work/build" --parallel "$(nproc)"
cmake --install "$work/build"
install -m0644 "$work/src/LICENSE" "$dest/LICENSE"
install -m0644 "$work/src/modules/core/3rdparty/SoftFloat/COPYING.txt" "$dest/SoftFloat-COPYING.txt"
touch "$stamp"
