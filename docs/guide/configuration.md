# Configuration

Gaze is configured with `/etc/gaze/config.toml`.

Most users only need to change camera source or security level.

## Default config

```toml
[security]
level = "medium"

[cameras]
rgb = "primary"
# ir = "/dev/video2"        # optional infrared camera
# emitter_enabled = false   # drive the IR emitter (requires ir)
dark_luma_threshold = 70

[auth]
abort_if_ssh = true
abort_if_lid_closed = true
require_confirmation = false

[enrollment]
max_templates = 3

[liveness]
enabled = true
threshold = 0.8
max_frames = 40
```

## Change security level

`level` (under `[security]`) controls model choice and match strictness.

| Level | Detector | Recognizer | Threshold | Notes |
|---|---|---|---|---|
| `low` | SCRFD-500M | MobileFaceNet | 0.30 | Fastest |
| `medium` | SCRFD-500M | MobileFaceNet | 0.40 | Default |
| `high` | SCRFD-10G | ResNet50 | 0.50 | More accurate |
| `maximum` | SCRFD-10G | ResNet50 | 0.60 | Most strict |
| `custom` | n/a | n/a | n/a | See below |

Practical guidance:

- `medium`: best starting point for most laptops
- `high`: use when false positives are unacceptable
- `low`: use on weaker hardware when speed is critical

### Custom level

```toml
[security]
level = "custom"
detector = "det_10g.onnx"
recognizer = "w600k_r50.onnx"
threshold = 0.55
```

## Select Camera Source

The default camera source is:

```toml
[cameras]
rgb = "primary"
```

`primary` uses GStreamer `pipewiresrc`. To pin Gaze to a specific PipeWire camera, use `gaze config` or set `rgb` to a GStreamer source:

```toml
[cameras]
rgb = "pipewiresrc target-object=<pipewire-target>"
```

Direct `/dev/video*` paths are not supported.

### Dark-frame rejection

Gaze rejects frames that are too dark before running face detection:

```toml
[cameras]
dark_luma_threshold = 70
```

With the default, a frame is skipped when its mean luminance (0-255, BT.601 weighted) falls below 70. Raise it to reject dimmer scenes, lower it to be more permissive.

## Infrared (IR) camera

Gaze can authenticate through a Windows Hello-style infrared camera instead of the RGB webcam. Point `ir` at the IR camera's `/dev/video*` node:

```toml
[cameras]
ir = "/dev/video2"
emitter_enabled = true
```

When `ir` is set, Gaze captures from that node (through GStreamer `v4l2src`) for both enrollment and verification, and `rgb` is ignored. Use `gaze discover` to list video devices and find the IR node:

```
$ gaze discover
/dev/video0  vid=0x04f2 pid=0xb604  no emitter profile
/dev/video2  vid=0x04f2 pid=0xb615  emitter: Chicony Integrated IR Camera ✓
```

### IR emitter

Many IR cameras keep their infrared LED off until told otherwise, leaving frames too dark to recognize. Set `emitter_enabled = true` and Gaze switches the emitter on during capture and off afterwards.

Gaze matches cameras by USB VID:PID against a small built-in table and also probes at runtime for the standard Microsoft Face Authentication control, so most Windows Hello cameras work with no manual setup. If `gaze discover` reports "no emitter profile" for your IR node and the emitter does not light, the camera needs a profile added under `gaze-core/ir-profiles/`.

On the IR path, liveness uses eye-motion analysis across frames; the RGB MiniFASNet model is not applied to infrared.

Driving the emitter needs read/write access to the IR `/dev/video*` node. The daemon runs as root and is a member of the `video` group, so the default `root:video` device permissions are sufficient; no extra udev rule is required.

## Authentication options

Gaze skips face authentication in sessions where the camera is unlikely or unsafe to use:

```toml
[auth]
abort_if_ssh = true
abort_if_lid_closed = true
require_confirmation = false
```

`abort_if_ssh` detects SSH sessions from the DBus caller process environment. `abort_if_lid_closed` reads ACPI lid state when available and is ignored on systems without a lid sensor.

Setting `require_confirmation = true` adds a manual intent check step after a successful face match (applies **only** to the `pam-gaze-grosshack` module). 

With `require_confirmation = true`:
- The password prompt still comes up immediately so you are never blocked.
- If face verification succeeds before you finish entering your password:
  - In a text-based (TTY) environment, it cancels the password prompt and asks for text confirmation ("Press Enter to confirm, Esc to cancel").
  - In a graphical Polkit environment:
    - On **GNOME** (with the Gaze Extension active), it hides the password field, activates the "Authenticate" button, and lets you confirm with a single click. If the extension is inactive, it bypasses confirmation entirely to avoid locking you out.
    - On **KDE Plasma & LXQt**, it prompts you to press "OK" to confirm.
    - On **Hyprland**, it prompts you to press "Authenticate" to confirm.
    - On other graphical environments, it prompts you to press "Enter" to confirm.

After changing config:

```bash
sudo systemctl restart gazed
```

## Storage paths

Storage locations are managed by the service setup and are not intended to be changed in config:

- User embeddings: `/var/lib/gaze/users`
- Downloaded models: `/var/cache/gaze`

Models are auto-downloaded on first run if missing.

## Enrollment behavior

```toml
[enrollment]
max_templates = 3
```

Increase this if auth is unreliable in varied lighting.

## Liveness Anti-Spoofing

```toml
[liveness]
enabled = true
threshold = 0.8
max_frames = 40
```

When enabled, Gaze runs a local MiniFASNet-V2 anti-spoofing model on the detected face crop after a recognition match. Authentication succeeds only when the face matches and either one frame reaches `threshold` or the best few frames show sustained near-threshold liveness.

`max_frames` caps how many valid face frames Gaze will try before returning no match.

## Recommended tuning workflow

1. Start with `[security] level = "medium"`
2. Enroll one profile: `gaze add-face default`
3. Test 5 to 10 times using `gaze auth --verbose`
4. If photo or screen spoofing is a concern, keep `[liveness] enabled = true`
5. If false accepts are too high, switch to `high`
6. If false rejects are too high, run `gaze refine-face default`
