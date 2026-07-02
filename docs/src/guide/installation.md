# Installation

Use one of these paths. The one-line installer enables GNOME lock screen auth for the current GNOME user when possible, and skips GNOME-specific packages on KDE Plasma and other non-GNOME desktops. Manual GNOME package installs still need GNOME settings commands afterward.

Supported installer targets: Ubuntu 24.04/25.10/26.04, Debian 13, Fedora 42/43/44, Arch Linux, and Arch-compatible AUR distributions such as Manjaro and CachyOS, on x86_64 and arm64.

## Path A: one-line installer (recommended)

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

This installs:

- the Gaze daemon and CLI
- `gaze-gui`
- the GNOME Shell extension package only when a GNOME desktop session is detected

It also configures package updates where needed, enables the `gazed` daemon, and tries to enable lock screen face unlock for the current GNOME user when applicable. On KDE Plasma and other non-GNOME desktops, it skips the GNOME extension package so it does not pull in GNOME Shell.

Desktop behavior:

- CLI, GUI, and normal PAM prompts work without the GNOME extension.
- If the installer detects KDE Plasma, it installs the base packages and points you to the PAM guide for login/lock integration.
- If you later want GNOME lock screen support, install the GNOME extension package manually from a GNOME session.
- GDM loads the extension from package defaults when the extension package is installed, but GDM login face auth stays disabled unless you explicitly enable it.

For non-interactive installs:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh -s -- --yes
```

## Path B: manual package install

Use this if you prefer to configure package sources yourself. Debian/Ubuntu and Fedora use Gundu Labs repositories. Arch Linux and Arch-compatible distributions such as Manjaro and CachyOS use the AUR packages.

If you are replacing an existing manual repository configuration, remove the current repo files first:

**Debian / Ubuntu:**
```bash
sudo rm -f /etc/apt/sources.list.d/gundulabs.list /usr/share/keyrings/gundulabs-archive-keyring.gpg
```

**Fedora:**
```bash
sudo rm -f /etc/yum.repos.d/gundulabs.repo /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
```

::: code-group

```bash [Debian/Ubuntu]
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.gpg \
  | sudo tee /usr/share/keyrings/gundulabs-archive-keyring.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] https://packages.gundulabs.com/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui
```

```bash [Fedora]
sudo rpm --import https://packages.gundulabs.com/keys/gundulabs-repo.asc
sudo tee /etc/yum.repos.d/gundulabs.repo >/dev/null <<'EOF'
[gundulabs]
name=Gundu Labs
baseurl=https://packages.gundulabs.com/rpm/fedora/$releasever/$basearch
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://packages.gundulabs.com/keys/gundulabs-repo.asc
EOF
sudo dnf makecache
sudo dnf install gaze gaze-gui
```

```bash [Arch Linux / Manjaro / CachyOS]
# Requires an AUR helper such as yay or paru. yay shown here.
yay -S --needed gaze-bin gaze-gui-bin
```

:::

## Path C: GUI-only via Flatpak

The Flatpak is published to the Gundu Labs repository. The signing key and repo
URL are embedded in the `.flatpakref`, so one command adds the remote and installs
the app:

```bash
flatpak install --from https://packages.gundulabs.com/flatpak/com.gundulabs.Gaze.flatpakref
```

This installs the sandboxed Gaze GUI only. It talks to the `gazed` daemon on the system bus, so you still need to install one of the system packages (Path A or B) for the daemon and PAM integration. Use this path when you want the GUI updated independently of the system package.

### Enable GNOME lock screen auth after manual install

Only run this on GNOME desktops where you want face unlock from the lock screen. First install the extension package for your distro, then enable it for your user; package managers do not safely change per-user extension settings.

::: code-group

```bash [Debian/Ubuntu]
sudo apt install gaze-gnome-extension
```

```bash [Fedora]
sudo dnf install gaze-gnome-extension
```

```bash [Arch Linux / Manjaro / CachyOS]
yay -S --needed gaze-gnome-extension-bin
```

:::

```bash
gnome-extensions enable gaze@gundulabs.com
gsettings set org.gnome.shell.extensions.gaze enable-face-authentication true
```

Log out and back in once after installing or updating the extension if the lock screen does not pick it up immediately. GDM login face auth stays disabled unless you explicitly enable it; see the [GNOME Extension guide](/guide/gnome) before doing that.

### KDE Plasma and other PAM-based desktops

The one-line installer detects KDE Plasma and intentionally skips `gaze-gnome-extension`, because that package depends on GNOME Shell. Use the base `gaze` package's PAM modules for login or lock-screen integration. See the [PAM guide](/guide/pam) and keep password fallback enabled while testing.

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
gaze doctor
gaze-gui --help
```

Run `gaze doctor` as your desktop user so it can inspect that user's PipeWire session and desktop integration.

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
