FROM fedora:latest
RUN dnf install -y --setopt=install_weak_deps=False \
    ca-certificates openssl-devel opencv-devel clang-devel pkgconfig libv4l-devel \
    pam-devel gtk4-devel libadwaita-devel curl git tar gzip \
  && dnf clean all
