# Installation

Use either of these paths. The one-line installer enables GNOME lock screen auth for the current GNOME user when possible. Manual package installs still need GNOME settings commands afterward.

Supported installer targets: Ubuntu 24.04/26.04, Debian 13, Fedora 42/43/44, Arch Linux, and Manjaro.

## Path A: one-line installer (recommended)

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

This installs:

- the Gaze daemon and CLI
- `gaze-gui`
- the GNOME Shell extension package

It also configures package updates where needed, enables the `gazed` daemon, and tries to enable lock screen face unlock for the current GNOME user.

GNOME behavior:

- CLI, GUI, and normal PAM prompts work without the GNOME extension.
- If the installer is not run from a GNOME desktop session, it prints the manual enable command instead.
- GDM loads the extension from package defaults, but GDM login face auth stays disabled unless you explicitly enable it.

For non-interactive installs:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh -s -- --yes
```

## Path B: manual package install

Use this if you prefer to configure package sources yourself. Debian/Ubuntu and Fedora use Gundu Labs repositories. Arch Linux and Manjaro use the AUR packages.

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
# Requires an AUR helper such as yay or paru. yay shown here.
yay -S --needed gaze-bin gaze-gui-bin gaze-gnome-extension-bin
```

:::

### Enable GNOME lock screen auth after manual install

Only run this on GNOME desktops where you want face unlock from the lock screen. The package is already installed by the default install commands above, but package managers do not safely change per-user extension settings.

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

Log out and back in once after installing or updating the extension if the lock screen does not pick it up immediately. GDM login face auth stays disabled unless you explicitly enable it; see the [GNOME Extension guide](/guide/gnome) before doing that.

### Enable face unlock for hyprlock

On Hyprland, install the `gaze-hyprlock` package (auto-installed by the one-line installer when Hyprland is detected) and point hyprlock at the Gaze PAM service. See the [Hyprland guide](/guide/hyprland).

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
