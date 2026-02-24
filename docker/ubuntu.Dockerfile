FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config build-essential libssl-dev libopencv-dev clang libclang-dev \
    libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev curl git \
  && rm -rf /var/lib/apt/lists/*
