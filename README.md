# Gaze

Gaze adds face authentication to Linux login and unlock.

> [!WARNING]
> Gaze is currently **not suitable for security-critical authentication**.
> The current pipeline can be spoofed with a simple photo of your face, including one shown on another screen.
> Use Gaze for convenience scenarios right now, not as a high-assurance identity factor.
> Liveness detection, IR camera support, and related anti-spoofing protections are planned for upcoming releases.

## Features

- Unlock and log in with your face on Linux
- Keep authentication on-device
- Use either a simple GUI (`gaze-gui`) or CLI (`gaze`)
- Tune security from fast to strict

## 5-minute quickstart

1. Install:

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh
```

2. Enroll your first face:

```bash
gaze add-face default
```

3. Test authentication:

```bash
gaze auth
```

4. Open the GUI (optional):

```bash
gaze-gui
```

If installation succeeded but auth fails, see: https://gaze.gundulabs.com/guide/troubleshooting

## What gets installed

- `gazed`: system daemon (DBus service)
- `gaze`: CLI client
- `gaze-gui`: GTK4 app
- PAM integration for login/lockscreen auth
- GNOME extension package (`gaze-gnome-extension`) for lock screen flow

## Install options

### Option A (recommended): one-line installer

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh
```

Supports Fedora/RHEL, Fedora Atomic desktops such as Silverblue, Debian/Ubuntu, and Arch/Manjaro.

### Option B: manually install from Gundu Labs repositories

Debian / Ubuntu:

```bash
curl -fsSL https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc \
  | gpg --dearmor \
  | sudo tee /usr/share/keyrings/gundulabs-packages.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/gundulabs-packages.gpg] https://packages.gundulabs.com/deb stable main" \
  | sudo tee /etc/apt/sources.list.d/gaze.list >/dev/null
sudo apt update
sudo apt install gaze gaze-gui gaze-gnome-extension
```

Fedora / RHEL:

```bash
sudo tee /etc/yum.repos.d/gaze.repo >/dev/null <<'EOF'
[gaze]
name=Gundu Labs Packages
baseurl=https://packages.gundulabs.com/rpm/x86_64
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc
EOF
sudo rpm --import https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc
sudo dnf install gaze gaze-gui gaze-gnome-extension
```

Arch / Manjaro:

```bash
sudo tee /etc/pacman.d/gaze-mirrorlist >/dev/null <<'EOF'
Server = https://packages.gundulabs.com/arch/x86_64
EOF
curl -fsSL https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc -o /tmp/gundulabs-packages.asc
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

### Option C: GUI via Flatpak

```bash
flatpak remote-add --if-not-exists --no-gpg-verify gundulabs https://packages.gundulabs.com/flatpak
flatpak install gundulabs com.gundulabs.Gaze
```

Note: Flatpak installs the GUI app. System login PAM integration is provided by distro packages.

For Fedora Silverblue, Kinoite, and similar atomic desktops, this is usually the best way to install the GUI. Use Flatpak, Distrobox, or whatever normal app workflow you already use instead of layering extra GUI packages unless you specifically want to.

## First-time setup checklist

Run these after install:

```bash
systemctl status gazed
gaze add-face default
gaze auth --verbose
```

Expected result from `gaze auth`: `Authenticated as: <face>`.

## Configuration (optional)

Main config file:

`/etc/gaze/config.toml`

Safe default:

```toml
level = "medium"

[cameras]
rgb = "/dev/video0"

[enrollment]
max_captures_per_face = 8
```

Details: https://gaze.gundulabs.com/guide/configuration

## Command cheat sheet

```bash
gaze add-face <name>                 # Enroll
gaze auth                            # Authenticate
gaze auth --verbose                  # Show similarity scores
gaze refine-face <name>              # Add more samples
gaze list-faces                      # List enrolled faces
gaze remove-face <name>              # Remove one face
gaze clear-user                      # Remove all face data for current user
gaze-gui                             # Open graphical app
```

## Documentation

Full documentation: https://gaze.gundulabs.com

## Building from source (for developers)

Install system dependencies first:

```bash
# Debian / Ubuntu
sudo apt install libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev

# Fedora / RHEL
sudo dnf install opencv-devel clang-devel libv4l-devel pam-devel gtk4-devel libadwaita-devel
```

Build workspace:

```bash
cargo build --workspace --release
```

## License

See `LICENSE`.
