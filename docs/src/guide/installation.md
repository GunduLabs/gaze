<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Installation

Use one of these paths. The one-line installer enables GNOME lock screen auth for the current GNOME user when possible, installs the KDE packages on KDE Plasma, and skips GNOME-specific packages on non-GNOME desktops. Manual GNOME package installs still need GNOME settings commands afterward.

Supported installer targets on x86_64 and arm64: Ubuntu 24.04/25.10/26.04, Debian 13 and 14 (forky, currently testing), Fedora 42/43/44 and compatible distributions (including image-based OSTree distros such as Fedora Silverblue, Kinoite, and Bazzite), openSUSE Tumbleweed (x86_64), Arch Linux, and Arch-compatible AUR distributions such as Manjaro and CachyOS.

## Path A: one-line installer (recommended)

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh
```

This installs:

- the Gaze daemon and CLI
- `gaze-gui`
- the GNOME Shell extension package only when a GNOME desktop session is detected

It also configures package updates where needed, enables the `gazed` daemon, and tries to enable lock screen face unlock for the current GNOME user when applicable. On KDE Plasma it installs `gaze-kde` instead, which wires up the lock screen. On other non-GNOME desktops it skips the GNOME extension package so it does not pull in GNOME Shell. On OSTree systems (Silverblue, Bazzite, Kinoite), the installer automatically uses `rpm-ostree` layering. On openSUSE Tumbleweed, it uses `zypper` and the Tumbleweed-specific Gundu Labs RPM repository.

Desktop behavior:

- CLI, GUI, and normal PAM prompts work without the GNOME extension.
- If the installer detects KDE Plasma, it installs `gaze-kde` alongside the base packages, so the lock screen starts face auth on its own and a Face Unlock entry appears in System Settings.
- If you later want GNOME lock screen support, install the GNOME extension package manually from a GNOME session.
- GDM loads the extension from package defaults when the extension package is installed, but GDM login face auth stays disabled unless you explicitly enable it.

For non-interactive installs:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sh -s -- --yes
```

## Path B: manual package install

Use this if you prefer to configure package sources yourself. Debian/Ubuntu, Fedora-compatible systems, and openSUSE Tumbleweed use Gundu Labs repositories. Arch Linux and Arch-compatible distributions such as Manjaro and CachyOS use the AUR packages.

Debian/Ubuntu packages are built per release, and each apt suite carries only the builds for that release: `noble` (Ubuntu 24.04), `questing` (Ubuntu 25.10), `resolute` (Ubuntu 26.04), `trixie` (Debian 13), and `forky` (Debian 14). The snippet below picks the suite matching your system; installing another release's package leaves `gazed` unable to load its OpenCV libraries.

Debian 14 (forky) is still testing, so its libraries keep moving. The `forky` packages are built against whatever OpenCV and GTK sonames testing carried at release time, and a soname bump in testing can leave `gazed` unable to start until the next Gaze release rebuilds against it. Reinstalling from the `forky` suite after such a bump picks up the rebuilt package.

If you are replacing an existing manual repository configuration, remove the current repo files first:

**Debian / Ubuntu:**
```bash
sudo rm -f /etc/apt/sources.list.d/gundulabs.list /usr/share/keyrings/gundulabs-archive-keyring.gpg
```

**Fedora and compatible DNF/rpm-ostree systems:**
```bash
sudo rm -f /etc/yum.repos.d/gundulabs.repo /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
```

**openSUSE Tumbleweed:**

```bash
sudo rm -f /etc/zypp/repos.d/gundulabs.repo /etc/pki/rpm-gpg/RPM-GPG-KEY-gundulabs
```

::: code-group

```bash [Debian/Ubuntu]
sudo mkdir -p --mode=0755 /usr/share/keyrings
curl -fsSL https://packages.gundulabs.com/keys/gundulabs-repo.gpg \
  | sudo tee /usr/share/keyrings/gundulabs-archive-keyring.gpg >/dev/null
suite="$(. /etc/os-release && echo "${VERSION_CODENAME:-$UBUNTU_CODENAME}")"
echo "deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/gundulabs-archive-keyring.gpg] https://packages.gundulabs.com/deb $suite main" \
  | sudo tee /etc/apt/sources.list.d/gundulabs.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui
```

```bash [Fedora and compatible]
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

```bash [Fedora OSTree (Silverblue / Bazzite / Kinoite)]
sudo tee /etc/yum.repos.d/gundulabs.repo >/dev/null <<'EOF'
[gundulabs]
name=Gundu Labs
baseurl=https://packages.gundulabs.com/rpm/fedora/$releasever/$basearch
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://packages.gundulabs.com/keys/gundulabs-repo.asc
EOF
sudo rpm-ostree install gaze gaze-gui
```

```bash [Fedora via Copr]
# Alternative to the Gundu Labs dnf repository above; do not enable both.
# Copr builds and signs these packages on Fedora's own builders.
sudo dnf install dnf-plugins-core
sudo dnf copr enable @gundulabs/gaze
sudo dnf install gaze gaze-gui
```

```bash [openSUSE Tumbleweed]
sudo rpm --import https://packages.gundulabs.com/keys/gundulabs-repo.asc
sudo tee /etc/zypp/repos.d/gundulabs.repo >/dev/null <<'EOF'
[gundulabs]
name=Gundu Labs
baseurl=https://packages.gundulabs.com/rpm/opensuse/tumbleweed/$basearch
enabled=1
autorefresh=1
type=rpm-md
gpgcheck=1
gpgkey=https://packages.gundulabs.com/keys/gundulabs-repo.asc
EOF
sudo zypper refresh
sudo zypper install gaze gaze-gui
```

The openSUSE RPM ships a `pam-config` definition and enables Gaze for `sudo`,
GDM, and other services that include `common-auth` in its post-install script.
To reapply the setting manually after changing PAM modules, run:

```bash
sudo pam-config --add --gaze
sudo pam-config --update
```

The one-line installer also reapplies this setting after the package install
when `pam-config` is available.

```bash [Arch Linux / Manjaro / CachyOS]
# Requires an AUR helper such as yay or paru. yay shown here.
yay -S --needed gaze-bin gaze-gui-bin
```

:::

::: warning Arch: install the `-bin` packages, not the release artifacts
On Arch, install Gaze only through the AUR packages above. The
`gaze-*.pkg.tar.zst` files attached to each GitHub release are the inputs the
AUR wrappers unpack, not a supported install path: they carry the plain package
names `gaze` and `gaze-gui`, and an unrelated AUR package is also named `gaze`
(a file watcher). Installing them with `pacman -U` leaves a foreign `gaze` that
the next `yay -Syu` or `paru -Syu` silently "upgrades" to the unrelated package,
removing `gazed`, the PAM modules, and the systemd unit. See
[Gaze disappears after an AUR helper upgrade](/guide/troubleshooting#gaze-disappears-after-an-aur-helper-upgrade-arch-linux)
if this already happened to you.
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

```bash [Fedora and compatible]
sudo dnf install gaze-gnome-extension
```

```bash [openSUSE Tumbleweed]
sudo zypper install gaze-gnome-extension
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

The one-line installer detects KDE Plasma and intentionally skips `gaze-gnome-extension`, because that package depends on GNOME Shell. It installs `gaze-kde` instead, which makes the KDE **lock screen** start face auth on its own with no key press and adds a Face Unlock entry to System Settings. Keep password fallback enabled while testing.

The KDE **login greeter** (Plasma Login Manager, or SDDM) is a separate program with no up-front biometric slot upstream, so face auth there starts when you submit the login form, exactly as a fingerprint reader does on the same screen. See the [KDE Plasma guide](/guide/kde#login-greeter).

For other PAM-based desktops, use the base `gaze` package's PAM modules and see the [PAM guide](/guide/pam).

### Enable face unlock for hyprlock

On Hyprland, install the `gaze-hyprlock` package (auto-installed by the one-line installer when Hyprland is detected) and point hyprlock at the Gaze PAM service. See the [Hyprland guide](/guide/hyprland).

## Path D: Nix and NixOS

Gaze ships a Nix flake with packages and a NixOS module that sets up the
daemon, D-Bus/polkit policies, and PAM declaratively:

```nix
# flake.nix inputs
inputs.gaze.url = "github:GunduLabs/gaze";
```

```nix
# NixOS configuration
imports = [ inputs.gaze.nixosModules.default ];
services.gaze = {
  enable = true;
  gui.enable = true;
};
```

On non-NixOS systems with Nix or home-manager you can install the CLI and GUI
from the flake, but the daemon and PAM integration still need a system package
(Path A or B). See the [Nix & NixOS guide](/guide/nixos) for module options,
GNOME/hyprlock integration, and home-manager usage.

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
