# Installation

Use either of these paths. The one-line installer enables GNOME lock screen auth for the current GNOME user when possible. Manual package installs still need one GNOME command afterward.

## Path A: one-line installer (recommended)

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

This installs:

- `gaze` (daemon + CLI)
- `gaze-gui`
- `gaze-gnome-extension`

It also configures your package repository for future updates, enables the `gazed` daemon, and tries to enable lock screen face unlock for the current GNOME user.

GNOME behavior:

- CLI, GUI, and normal PAM prompts work without the GNOME extension.
- If the installer is not run from a GNOME desktop session, it prints the manual enable command instead.
- GDM login face auth is separate and stays disabled unless you explicitly enable it.

For non-interactive installs:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh -s -- --yes
```

## Path B: install from Gundu Labs repositories

Use this if you prefer manual repository setup.

::: code-group

```bash [Debian/Ubuntu]
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.gpg \
  | sudo tee /usr/share/keyrings/gundulabs-archive-keyring.gpg >/dev/null
. /etc/os-release
printf 'deb [signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] https://packages.gundulabs.com/deb %s main\n' "$VERSION_CODENAME" \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui gaze-gnome-extension
```

```bash [Fedora]
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

### Enable GNOME lock screen auth after manual install

Only run this on GNOME desktops where you want face unlock from the lock screen. The package is already installed by the default install commands above, but package managers do not safely change per-user extension settings.

```bash
gnome-extensions enable gaze@gundulabs.com
```

Log out and back in once after installing or updating the extension if the lock screen does not pick it up immediately. GDM login face auth stays disabled unless you explicitly enable it; see the [GNOME Extension guide](/guide/gnome) before doing that.

## Restart after install

After installation (any method), reboot once to ensure all system-level changes are fully applied.

```bash
sudo reboot
```

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

See the [Development guide](/guide/development) for source builds, tests, and packaging workflows.
