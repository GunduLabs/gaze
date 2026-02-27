# Gaze

Facial authentication daemon for Linux. Uses InsightFace ONNX models (SCRFD detection + ArcFace recognition) to provide face-based login via PAM and a DBus interface.

## Features

- Fast, local face recognition — no cloud dependency
- PAM integration for system login (GDM, lightdm, etc.)
- DBus interface (`org.gaze.Auth`) for third-party integration
- GTK4/Adwaita GUI for enrollment
- Configurable security levels (model + similarity threshold)
- Models auto-downloaded from InsightFace on first run

## Requirements

**Rust toolchain** (2024 edition) and the following system libraries:

```
libopencv-dev libclang-dev libv4l-dev libpam0g-dev libgtk-4-dev libadwaita-1-dev
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

Install `nfpm` (required for packaging):

```bash
go install github.com/goreleaser/nfpm/v2/cmd/nfpm@latest
export PATH="$PATH:$(go env GOPATH)/bin"
```

Rebuild, repackage, reinstall, and configure everything in one shot (resets the config so the package version is laid down fresh):

```bash
sudo rm -f /etc/gaze/config.toml && \
cargo build --workspace --release && \
VERSION=0.0.1 ARCH=x86_64 nfpm pkg -f packaging/nfpm.yaml --packager rpm --target /tmp/ && \
VERSION=0.0.1 ARCH=x86_64 nfpm pkg -f packaging/nfpm_gui.yaml --packager rpm --target /tmp/ && \
VERSION=0.0.1 ARCH=x86_64 nfpm pkg -f packaging/nfpm_gnome_extension.yaml --packager rpm --target /tmp/ && \
sudo rpm -Uvh --force /tmp/gaze-0.0.1-1.x86_64.rpm /tmp/gaze-gui-0.0.1-1.x86_64.rpm /tmp/gaze-gnome-extension-0.0.1-1.x86_64.rpm && \
sudo systemctl enable --now gazed && \
sudo authselect select custom/gaze --force && \
gnome-extensions enable gaze@gundulabs.com
```

## Workspace

| Crate | Description |
|---|---|
| `gaze` | Daemon (`gazed`) and CLI (`gaze`) |
| `gaze_core` | Shared camera, detection, config, DBus types |
| `gaze_gui` | GTK4/Adwaita enrollment and auth GUI |
| `pam_gaze` | PAM module (`libpam_gaze.so`) |
| `pam_gaze_grosshack` | PAM compatibility shim |
| `pam_gaze_core` | Core PAM logic |

## Installation

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

4. Install the PAM module:

```bash
# Fedora / RHEL (x86_64)
sudo cp target/release/libpam_gaze.so /lib64/security/pam_gaze.so

# Debian / Ubuntu
sudo cp target/release/libpam_gaze.so /lib/x86_64-linux-gnu/security/pam_gaze.so
```

5. Add to your PAM config (e.g. `/etc/pam.d/gdm-password`):

```
auth sufficient pam_gaze.so
```

6. Enable face authentication via authselect (Fedora/RHEL):

```bash
sudo authselect select custom/gaze
```

This configures `system-auth` and `password-auth` to include `pam_gaze.so`, covering both login and lock screen unlock via GDM.

7. Enable the GNOME Shell extension (for lock screen support):

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

```bash
gaze auth                        # Authenticate current user
gaze add-face <name>             # Enroll a new face
gaze refine-face <name>          # Add more samples to existing face
gaze remove-face <name>          # Remove a specific face
gaze clear-user                  # Remove all faces for current user
```

The CLI communicates with the running daemon over DBus.

### GUI

Launch `gaze-gui` for a graphical enrollment and authentication interface.

## How It Works

```
Camera frame → SCRFD face detection → Umeyama alignment (112×112)
→ ResNet50 / MobileFaceNet embedding → cosine similarity → auth result
```

Face embeddings are stored as binary files at:
```
/var/lib/gaze/users/{username}/{face_name}/{uuid}.bin
```

## License

See [LICENSE](LICENSE).
