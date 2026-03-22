# Installation

Use one of these paths.

## Path A: one-line installer (recommended)

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

This installs:

- `gaze` (daemon + CLI)
- `gaze-gui`
- `gaze-gnome-extension`

It also configures your package repository for future updates.

## Path B: install from Gundu Labs repositories

Use this if you prefer manual repository setup.

::: code-group

```bash [Debian/Ubuntu]
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.gpg \
  | sudo tee /usr/share/keyrings/gundulabs-archive-keyring.gpg >/dev/null
curl -fsSL https://packages.gundulabs.com/setup/deb/gundulabs.list \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui gaze-gnome-extension
```

```bash [Fedora/RHEL]
sudo rpm --import https://packages.gundulabs.com/keys/gundulabs-repo.asc
sudo curl -fsSL https://packages.gundulabs.com/setup/rpm/gundulabs.repo \
  -o /etc/yum.repos.d/gundulabs.repo
sudo dnf makecache
sudo dnf install gaze gaze-gui gaze-gnome-extension
```

```bash [Arch Linux / Manjaro]
sudo tee /etc/pacman.d/gaze-mirrorlist >/dev/null <<'EOF'
Server = https://packages.gundulabs.com/arch/x86_64
EOF
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.asc -o /tmp/gundulabs-packages.asc
sudo pacman-key --add /tmp/gundulabs-packages.asc
sudo pacman-key --lsign-key "$(gpg --show-keys --with-colons /tmp/gundulabs-packages.asc | awk -F: '/^fpr:/ {print $10; exit}')"
rm -f /tmp/gundulabs-packages.asc
sudo tee -a /etc/pacman.conf >/dev/null <<'EOF'
[gaze]
SigLevel = Required DatabaseOptional
Include = /etc/pacman.d/gaze-mirrorlist
EOF
sudo pacman -Sy gaze gaze-gui gaze-gnome-extension
```

:::

## Path C: GUI-only via Flatpak

```bash
flatpak remote-add --if-not-exists --no-gpg-verify gundulabs https://packages.gundulabs.com/flatpak
flatpak install gundulabs com.gundulabs.Gaze
```

Use this if you only want the GUI app.

## Verify installation

```bash
systemctl status gazed
gaze --version
gaze-gui --help
```

If daemon is inactive:

```bash
sudo systemctl enable --now gazed
```

## First run

```bash
gaze add-face default
gaze auth --verbose
```

## Development and source builds

See the [Development guide](/guide/development) for source builds, tests, packaging, and Flatpak development workflows.
