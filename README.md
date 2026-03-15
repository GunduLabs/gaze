# Gaze

Facial authentication for Linux.

## Features

- Fast, local face recognition — no cloud dependency
- PAM integration for system login (GDM, lightdm, etc.)
- DBus interface (`org.gaze.Auth`) for third-party integration
- GTK4/Adwaita GUI for enrollment and testing
- Configurable security levels (model + similarity threshold)
- Models auto-downloaded from InsightFace on first run

## Requirements

**Rust toolchain** (2024 edition) and the following system libraries:

```bash
# Debian / Ubuntu
sudo apt install libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev

# Fedora / RHEL
sudo dnf install opencv-devel clang-devel libv4l-devel pam-devel gtk4-devel libadwaita-devel
```

## Building

```bash
cargo build --workspace --release        # Build everything
cargo build --bin gazed --release        # Daemon only
cargo build --bin gaze --release         # CLI only
cargo build -p gaze_gui --release        # GTK4 GUI
cargo build -p pam_gaze --release        # PAM module (libpam_gaze.so)
```

## Development

Install `cargo-nfpm` (required for packaging):

```bash
cargo install cargo-nfpm --locked
```

Build Flatpak package for the GUI:

```bash
chmod +x scripts/build-flatpak.sh
scripts/build-flatpak.sh
```

Rebuild, repackage, reinstall, and re-configure everything in one shot (resets the config so the packaged version is applied fresh):

```bash
./dev-reinstall.sh
```

On Wayland, GNOME Shell must be restarted (log out and back in) before it picks up newly installed system extensions. After logging back in, run:

```bash
gnome-extensions enable gaze@gundulabs.com
```

## Workspace

| Crate | Description |
|---|---|
| `gaze` | Daemon (`gazed`) and CLI (`gaze`) |
| `gaze_core` | Shared camera, detection, config, and DBus types |
| `gaze_gui` | GTK4/Adwaita enrollment and auth GUI |
| `pam_gaze` | PAM module (`libpam_gaze.so`) |
| `pam_gaze_core` | Core PAM logic |
| `pam_gaze_grosshack` | PAM compatibility shim |

## Installation

### Install from Gundu Labs package repositories

This guide assumes a shared package endpoint at `https://packages.gundulabs.com` (backed by Cloudflare R2).

```bash
# Debian/Ubuntu
curl -fsSL https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc \
	| gpg --dearmor \
	| sudo tee /usr/share/keyrings/gundulabs-packages.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/gundulabs-packages.gpg] https://packages.gundulabs.com/deb stable main" | sudo tee /etc/apt/sources.list.d/gaze.list
sudo apt update
sudo apt install gaze gaze-gui gaze-gnome-extension

# Fedora/RHEL
sudo tee /etc/yum.repos.d/gaze.repo >/dev/null <<'EOF'
[gaze]
name=Gaze Packages
baseurl=https://packages.gundulabs.com/rpm/x86_64
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc
EOF
sudo rpm --import https://packages.gundulabs.com/PACKAGE-SIGNING-KEY.asc
sudo dnf install gaze gaze-gui gaze-gnome-extension

# Arch Linux
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

### Manual install from local build artifacts

### Flatpak GUI install

```bash
flatpak remote-add --if-not-exists --no-gpg-verify gundulabs https://packages.gundulabs.com/flatpak
flatpak install gundulabs com.gundulabs.Gaze
```

1. Install the binaries and enable the systemd service:

```bash
sudo cp target/release/gazed /usr/bin/gazed
sudo cp target/release/gaze /usr/bin/gaze
sudo cp target/release/gaze-gui /usr/bin/gaze-gui
sudo cp dist/gazed.service /etc/systemd/system/
sudo systemctl enable --now gazed
```

2. Install the DBus policy:

```bash
sudo cp dist/org.gaze.Auth.conf /etc/dbus-1/system.d/
```

3. Install the config:

```bash
sudo mkdir -p /etc/gaze
sudo cp dist/config.toml /etc/gaze/config.toml
```

4. Install the PAM modules:

```bash
# Fedora / RHEL (x86_64)
sudo cp target/release/libpam_gaze.so /usr/lib64/security/pam_gaze.so
sudo cp target/release/libpam_gaze_grosshack.so /usr/lib64/security/pam_gaze_grosshack.so

# Debian / Ubuntu
sudo cp target/release/libpam_gaze.so /lib/x86_64-linux-gnu/security/pam_gaze.so
sudo cp target/release/libpam_gaze_grosshack.so /lib/x86_64-linux-gnu/security/pam_gaze_grosshack.so

# Arch Linux
sudo cp target/release/libpam_gaze.so /usr/lib/security/pam_gaze.so
sudo cp target/release/libpam_gaze_grosshack.so /usr/lib/security/pam_gaze_grosshack.so
```

5. Enable face authentication:

```bash
# Fedora / RHEL — select the vendor authselect profile:
sudo authselect select vendor/gaze --force

# Debian / Ubuntu — register with pam-auth-update:
sudo cp dist/pam-configs/gaze dist/pam-configs/gaze-simultaneous /usr/share/pam-configs/
sudo pam-auth-update --package
```

6. Enable the GNOME Shell extension (for lock screen support):

```bash
gnome-extensions enable gaze@gundulabs.com
```

The extension hooks into GDM to trigger face auth on the lock screen using `/etc/pam.d/gdm-face`. It also installs a SELinux policy that allows GDM to access the camera.

## Configuration

`/etc/gaze/config.toml`:

```toml
# Preset security levels:
#   low      — MobileFaceNet + SCRFD-500M, threshold 0.3  (fastest)
#   medium   — MobileFaceNet + SCRFD-500M, threshold 0.4  (default)
#   high     — ResNet50 + SCRFD-10G, threshold 0.5
#   maximum  — ResNet50 + SCRFD-10G, threshold 0.6

level = "medium"

[cameras]
rgb = "/dev/video0"

[storage]
users_dir = "/var/lib/gaze/users"
models_dir = "/opt/gaze/models"

[enrollment]
max_captures_per_face = 8
```

Models are downloaded automatically to `models_dir` on first run.

## Usage

### CLI

All commands communicate with the running `gazed` daemon over DBus. Each accepts `-u <user>` to target a specific user instead of `$USER`.

```bash
gaze auth                            # Authenticate the current user
gaze auth --verbose                  # Show a per-face similarity score table
gaze auth --perf                     # Print step-by-step timing metrics
gaze add-face <name>                 # Enroll a new face (guided multi-angle capture)
gaze refine-face <name>              # Add more captures to an existing face
gaze list-faces                      # List all enrolled faces for the current user
gaze remove-face <name>              # Delete a specific enrolled face
gaze rename-face <old-name> <new-name>  # Rename an enrolled face
gaze clear-user                      # Remove all faces and data for the current user
```

Auth results:
- **Green ✓** — authenticated (`✓ Authenticated as: <face> (<pct>%, <ms>ms)`)
- **Red ✗** — access denied (`✗ Access Denied. (<ms>ms)`)

While scanning, the spinner shows real-time feedback if no face is detected or the face is clipped. The CLI communicates with the running daemon over DBus.

### GUI

Launch `gaze-gui` for a graphical enrollment and authentication interface. The test authentication button shows a color-coded result label (green/red) matching the CLI output.

## How It Works

```
Camera frame → SCRFD face detection → Umeyama alignment (112×112)
→ ResNet50 / MobileFaceNet embedding → cosine similarity → auth result
```

Face embeddings are stored as binary files at:
```
/var/lib/gaze/users/{username}/{face_name}/{uuid}.bin
```

Each file is a raw `f32` array (512 floats = 2048 bytes). Multiple captures per face improve robustness — all embeddings for a face are scored individually and the best match wins.

## License

See [LICENSE](LICENSE).
