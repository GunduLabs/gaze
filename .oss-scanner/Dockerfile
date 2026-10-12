# SPDX-FileCopyrightText: 2026 Gundu Labs
# SPDX-License-Identifier: GPL-3.0-or-later

FROM ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        git \
        ca-certificates \
        curl \
        unzip \
        build-essential \
        clang \
        libclang-dev \
        libcairo2-dev \
        libglib2.0-dev \
        libgdk-pixbuf-2.0-dev \
        libgstreamer1.0-dev \
        libgstreamer-plugins-base1.0-dev \
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-pipewire \
        libpango1.0-dev \
        libgraphene-1.0-dev \
        libopencv-dev \
        libv4l-dev \
        libpam0g-dev \
        libtss2-dev \
        libgtk-4-dev \
        libadwaita-1-dev \
        libssl-dev \
        pkg-config \
        dbus \
        polkitd \
        gdb \
        strace \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --profile minimal
ENV PATH=/root/.cargo/bin:$PATH

COPY . /src
WORKDIR /src

RUN cargo fetch --locked

ENV CARGO_PROFILE_RELEASE_DEBUG=true \
    CARGO_PROFILE_RELEASE_STRIP=false \
    RUSTFLAGS="-C force-frame-pointers=yes"

RUN bash scripts/prepare-ort.sh 1.31.0 "$(uname -m)"

RUN cargo build --locked --release -p gazed \
    && cargo build --locked --release -p gaze-cli -p gaze-gui -p pam-gaze -p pam-gaze-grosshack \
    && scripts/check-pam-link.sh target/release/libpam_gaze.so target/release/libpam_gaze_grosshack.so

RUN set -eux; \
    models=/var/cache/gaze; \
    mkdir -p "$models"; \
    chmod 700 "$models"; \
    base=https://github.com/deepinsight/insightface/releases/download/v0.7; \
    curl -fsSL --retry 3 "$base/buffalo_sc.zip" -o /tmp/buffalo_sc.zip; \
    curl -fsSL --retry 3 "$base/buffalo_l.zip" -o /tmp/buffalo_l.zip; \
    printf '%s  %s\n' \
        57d31b56b6ffa911c8a73cfc1707c73cab76efe7f13b675a05223bf42de47c72 /tmp/buffalo_sc.zip \
        80ffe37d8a5940d59a7384c201a2a38d4741f2f3c51eef46ebb28218a7b0ca2f /tmp/buffalo_l.zip \
        | sha256sum -c -; \
    unzip -j -o /tmp/buffalo_sc.zip '*.onnx' -d "$models"; \
    unzip -j -o /tmp/buffalo_l.zip '*.onnx' -d "$models"; \
    rm /tmp/buffalo_sc.zip /tmp/buffalo_l.zip; \
    curl -fsSL --retry 3 \
        https://huggingface.co/garciafido/minifasnet-v2-anti-spoofing-onnx/resolve/main/minifasnet_v2.onnx \
        -o "$models/minifasnet_v2.onnx"; \
    curl -fsSL --retry 3 \
        https://storage.openvinotoolkit.org/repositories/open_model_zoo/public/2022.1/open-closed-eye-0001/open_closed_eye.onnx \
        -o "$models/open_closed_eye.onnx"; \
    cd "$models"; \
    printf '%s  %s\n' \
        5e4447f50245bbd7966bd6c0fa52938c61474a04ec7def48753668a9d8b4ea3a det_500m.onnx \
        9cc6e4a75f0e2bf0b1aed94578f144d15175f357bdc05e815e5c4a02b319eb4f w600k_mbf.onnx \
        5838f7fe053675b1c7a08b633df49e7af5495cee0493c7dcf6697200b85b5b91 det_10g.onnx \
        4c06341c33c2ca1f86781dab0e829f88ad5b64be9fba56e56bc9ebdefc619e43 w600k_r50.onnx \
        d7b3cd9ba8a7ceb13baa8c4720902e27ca3112eff52f926c08804af6b6eecc7b minifasnet_v2.onnx \
        4daa100034482525a26c9afb9297c16580a531189e66e3d2b2ac7d32becfd593 open_closed_eye.onnx \
        | sha256sum -c -; \
    chmod 600 "$models"/*.onnx

RUN set -eux; \
    install -m 0755 target/release/gazed /usr/bin/gazed; \
    install -m 0755 target/release/gaze /usr/bin/gaze; \
    install -D -m 0644 target/release/libonnxruntime.so /usr/lib/gaze/libonnxruntime.so; \
    install -D -m 0644 packaging/config/config.toml /etc/gaze/config.toml; \
    install -D -m 0644 packaging/config/com.gundulabs.Gaze.conf /etc/dbus-1/system.d/com.gundulabs.Gaze.conf; \
    install -D -m 0644 packaging/config/com.gundulabs.gaze.policy /usr/share/polkit-1/actions/com.gundulabs.gaze.policy; \
    multiarch="$(gcc -dumpmachine)"; \
    for dir in "/lib/$multiarch/security" /usr/lib/security; do \
        install -D -m 0644 target/release/libpam_gaze.so "$dir/pam_gaze.so"; \
        install -D -m 0644 target/release/libpam_gaze_grosshack.so "$dir/pam_gaze_grosshack.so"; \
    done; \
    install -d -m 0700 /var/lib/gaze

RUN (cargo test --locked --workspace --release || echo "WARNING: cargo test reported failures") \
    && (bash scripts/test-keyring-pam.sh || echo "WARNING: keyring PAM harness reported failures") \
    && (bash scripts/test-kwallet-pam.sh || echo "WARNING: kwallet PAM harness reported failures")
