# Configuration

Gaze is configured with `/etc/gaze/config.toml`.

Most users only need to change camera device or security level.

## Default config

```toml
level = "medium"

[cameras]
rgb = "/dev/video0"

[storage]
users_dir = "/var/lib/gaze/users"
models_dir = "/opt/gaze/models"

[enrollment]
max_captures_per_face = 8
```

## Change security level

`level` controls model choice and match strictness.

| Level | Detector | Recognizer | Threshold | Notes |
|---|---|---|---|---|
| `low` | SCRFD-500M | MobileFaceNet | 0.30 | Fastest |
| `medium` | SCRFD-500M | MobileFaceNet | 0.40 | Default |
| `high` | SCRFD-10G | ResNet50 | 0.50 | More accurate |
| `maximum` | SCRFD-10G | ResNet50 | 0.60 | Most strict |
| `custom` | — | — | — | See below |

Practical guidance:

- `medium`: best starting point for most laptops
- `high`: use when false positives are unacceptable
- `low`: use on weaker hardware when speed is critical

### Custom level

```toml
level = "custom"
detector = "det_10g.onnx"
recognizer = "w600k_r50.onnx"
threshold = 0.55
```

## Select camera device

List camera devices:

```bash
ls /dev/video*
```

Then set:

```toml
[cameras]
rgb = "/dev/video0"
```

If you use an external USB webcam, it may appear as `/dev/video1` or higher.

After changing config:

```bash
sudo systemctl restart gazed
```

## Storage paths

```toml
[storage]
users_dir = "/var/lib/gaze/users"
models_dir = "/opt/gaze/models"
```

What these do:

- `users_dir`: enrolled face embeddings per user
- `models_dir`: downloaded ONNX models used by detector/recognizer

Models are auto-downloaded on first run if missing.

## Enrollment behavior

```toml
[enrollment]
max_captures_per_face = 8
```

Increase this if auth is unreliable in varied lighting.

## Recommended tuning workflow

1. Start with `level = "medium"`
2. Enroll one profile: `gaze add-face default`
3. Test 5 to 10 times using `gaze auth --verbose`
4. If false accepts are too high, switch to `high`
5. If false rejects are too high, run `gaze refine-face default`
