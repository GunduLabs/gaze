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
max_templates = 2

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

Gaze can authenticate through a Windows Hello-style infrared camera alongside the RGB webcam. Point `ir` to the IR camera's `/dev/video*` node or a GStreamer/PipeWire source string:

```toml
[cameras]
ir = "/dev/video2"
emitter_enabled = false
```

When `ir` is set, Gaze enrolls and verifies IR templates in addition to RGB templates when both cameras are configured. Hybrid auth policy depends on the security level: `low` accepts either spectrum, `medium`/`high` use IR as a fallback when RGB is dark or unavailable, and `maximum` requires both. Use `gaze discover` to list video devices, check if their emitter profiles are supported, and see which node is configured:

```
$ gaze discover
/dev/video0  vid=0x04f2 pid=0xb604  no emitter profile
/dev/video2  vid=0x04f2 pid=0xb615  emitter: Chicony Integrated IR Camera ✓  ← configured (cameras.ir)
```

### IR emitter blaster

Many IR cameras automatically light their infrared LED when streaming starts. If yours does not, set `emitter_enabled = true` to manually drive the emitter during authentication.

Gaze resolves the underlying `/dev/video*` node from the PipeWire camera, matches it by USB VID:PID against a small built-in table, and also probes at runtime for the standard Microsoft Face Authentication control to send UVC toggle requests. If `gaze discover` reports "no emitter profile" and the emitter does not light even with `emitter_enabled = true`, the camera needs a profile added under `gaze-core/ir-profiles/`.

On the IR path, liveness uses eye-motion analysis across frames; the RGB MiniFASNet model is not applied to infrared.

Driving the emitter blaster needs read/write access to the IR `/dev/video*` node. The daemon runs as root and is a member of the `video` group, so the default `root:video` device permissions are sufficient; no extra udev rule is required.

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
max_templates = 2
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
