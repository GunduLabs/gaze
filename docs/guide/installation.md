# Installation

## 1. Install binaries and enable the daemon

```bash
sudo cp target/release/gazed /usr/bin/gazed
sudo cp target/release/gaze /usr/bin/gaze
sudo cp target/release/gaze-gui /usr/bin/gaze-gui
sudo cp dist/gazed.service /etc/systemd/system/
sudo systemctl enable --now gazed
```

## 2. Install the DBus policy

```bash
sudo cp dist/org.gaze.Auth.conf /etc/dbus-1/system.d/
```

## 3. Install the config

```bash
sudo mkdir -p /etc/gaze
sudo cp dist/config.toml /etc/gaze/config.toml
```

## 4. Install the PAM module

::: code-group

```bash [Fedora/RHEL (x86_64)]
sudo cp target/release/libpam_gaze.so /lib64/security/pam_gaze.so
```

```bash [Debian/Ubuntu]
sudo cp target/release/libpam_gaze.so /lib/x86_64-linux-gnu/security/pam_gaze.so
```

:::

## 5. Configure PAM

Add to your PAM config (e.g. `/etc/pam.d/gdm-password`):

```
auth sufficient pam_gaze.so
```

## 6. Enable via authselect (Fedora/RHEL)

```bash
sudo authselect select custom/gaze
```

This configures `system-auth` and `password-auth` to include `pam_gaze.so`, covering both login and lock screen unlock via GDM.

## 7. Enable the GNOME Shell extension

```bash
gnome-extensions enable gaze@gundulabs.com
```

The extension hooks into GDM to trigger face auth on the lock screen using `/etc/pam.d/gdm-face`. It also installs a SELinux policy that allows GDM to access the camera.

::: warning Wayland note
On Wayland, GNOME Shell must be restarted (log out and back in) before it picks up a newly installed system extension.
:::

## One-shot rebuild & reinstall (development)

Requires `nfpm`:

```bash
go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest
export PATH="$PATH:$(go env GOPATH)/bin"
```

Then:

```bash
sudo rm -f /etc/gaze/config.toml && \
cargo build --workspace --release && \
VERSION=0.0.1 ARCH=x86_64 nfpm pkg -f packaging/nfpm.yaml --packager rpm --target /tmp/ && \
VERSION=0.0.1 ARCH=x86_64 nfpm pkg -f packaging/nfpm_gui.yaml --packager rpm --target /tmp/ && \
VERSION=0.0.1 ARCH=x86_64 nfpm pkg -f packaging/nfpm_gnome_extension.yaml --packager rpm --target /tmp/ && \
sudo rpm -Uvh --force /tmp/gaze-0.0.1-1.x86_64.rpm /tmp/gaze-gui-0.0.1-1.x86_64.rpm /tmp/gaze-gnome-extension-0.0.1-1.x86_64.rpm && \
sudo systemctl enable --now gazed && \
sudo authselect select custom/gaze --force
```
