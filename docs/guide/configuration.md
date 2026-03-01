# Configuration

Gaze is configured via `/etc/gaze/config.toml`.

## Full example

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

## Security levels

The `level` field selects the detector + recognizer model pair and similarity threshold:

| Level | Detector | Recognizer | Threshold | Notes |
|---|---|---|---|---|
| `low` | SCRFD-500M | MobileFaceNet | 0.30 | Fastest |
| `medium` | SCRFD-500M | MobileFaceNet | 0.40 | Default |
| `high` | SCRFD-10G | ResNet50 | 0.50 | More accurate |
| `maximum` | SCRFD-10G | ResNet50 | 0.60 | Most strict |
| `custom` | — | — | — | See below |

### Custom level

```toml
level = "custom"
detector = "det_10g.onnx"
recognizer = "w600k_r50.onnx"
threshold = 0.55
```

## Camera

```toml
[cameras]
rgb = "/dev/video0"   # Path to your RGB webcam device
```

## Storage

```toml
[storage]
users_dir = "/var/lib/gaze/users"   # Where face embeddings are stored
models_dir = "/opt/gaze/models"     # Where ONNX models are cached
```

Models are downloaded automatically from InsightFace on first run if not present.

## Enrollment

```toml
[enrollment]
max_captures_per_face = 8   # Number of angles captured per face during enrollment
```
