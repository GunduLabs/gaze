ARG BASE=archlinux:latest
FROM ${BASE}
RUN echo "DisableSandbox" >> /etc/pacman.conf \
  && pacman -Sy --noconfirm \
  && pacman -S --noconfirm --ask=4 --needed \
    base-devel openssl libssh2 opencv pam dbus gtk4 libadwaita clang pkg-config v4l-utils curl git \
  && pacman -Scc --noconfirm \
  && rm -rf /usr/share/doc/* /usr/share/man/* /tmp/*
