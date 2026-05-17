# Configuration

Gaze is configured with `/etc/gaze/config.toml`.

Most users only need to change camera source or security level.

## Default config

```toml
[security]
level = "medium"

[cameras]
rgb = "primary"
dark_threshold = 0.6
dark_pixel_value = 10

[enrollment]
max_templates = 3
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
dark_threshold = 0.6
dark_pixel_value = 10
```

With the defaults, a frame is skipped when at least 60% of pixels have luminance below 10.

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

## Recommended tuning workflow

1. Start with `[security] level = "medium"`
2. Enroll one profile: `gaze add-face default`
3. Test 5 to 10 times using `gaze auth --verbose`
4. If false accepts are too high, switch to `high`
5. If false rejects are too high, run `gaze refine-face default`
