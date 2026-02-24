ARG BASE=archlinux:latest
FROM ${BASE}
RUN echo "DisableSandbox" >> /etc/pacman.conf \
  && pacman -Syu --noconfirm \
  && pacman -S --noconfirm --ask=4 \
    base-devel opencv pam dbus gtk4 libadwaita clang pkg-config v4l-utils curl git \
  && pacman -Scc --noconfirm
