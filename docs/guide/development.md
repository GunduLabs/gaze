# Development

This page covers source builds, tests, and packaging workflows for contributors.

## Prerequisites

- Rust 1.70+ (or install via `rustup`)
- `just` (https://github.com/casey/just) for task automation
- `nfpm` (https://nfpm.goreleaser.com) for packaging

::: code-group

```bash [Debian/Ubuntu]
sudo apt install build-essential libopencv-dev libclang-dev libv4l-dev \
  libpam0g-dev libgtk-4-dev libadwaita-1-dev
```

```bash [Fedora/RHEL]
sudo dnf install @development-tools opencv-devel clang-devel libv4l-devel \
  pam-devel gtk4-devel libadwaita-devel
```

```bash [Arch Linux / Manjaro]
sudo pacman -S base-devel opencv clang libv4l pam gtk4 libadwaita
```

:::

## Setup

```bash
git clone https://github.com/gundulabs/gaze
cd gaze
just --list
```

## Build and test rust components

```bash
just build-rust
just test
just lint
just fmt-check
```

## Packaging

```bash
just package <deb | rpm | archlinux>
```

Package output:

- `dist/packages/`

## Cleaning build artifacts

```bash
just clean
```
