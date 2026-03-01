# How It Works

## Pipeline

```
Camera frame → SCRFD face detection → Umeyama alignment (112×112)
→ ResNet50 / MobileFaceNet embedding → cosine similarity → auth result
```

1. **Camera frame** — a raw frame is captured from the configured V4L2 device
2. **SCRFD face detection** — a lightweight ONNX model locates faces and their 5-point landmarks in the frame
3. **Umeyama alignment** — the detected face is warped to ArcFace-standard 112×112 alignment using the landmarks
4. **Embedding** — either MobileFaceNet or ResNet50 (depending on security level) produces a 512-dim face embedding
5. **Cosine similarity** — the embedding is compared against all stored embeddings for the user; the best match is returned

## Embedding storage

Face embeddings are stored as binary files:

```
/var/lib/gaze/users/{username}/{face_name}/{uuid}.bin
```

Each file is a raw `f32` array (512 floats = 2048 bytes). Multiple captures per face improve robustness — all embeddings for a face are scored individually and the best match wins.

## Workspace

| Crate | Description |
|---|---|
| `gaze` | Daemon (`gazed`) and CLI (`gaze`) |
| `gaze_core` | Shared camera, detection, config, DBus types |
| `gaze_gui` | GTK4/Adwaita enrollment and auth GUI |
| `pam_gaze` | PAM module (`libpam_gaze.so`) |
| `pam_gaze_core` | Core PAM logic |
| `pam_gaze_grosshack` | PAM compatibility shim |

## DBus interface

The daemon registers as `org.gaze.Auth` at `/org/gaze/Auth` on the system bus. This interface is used by the CLI, GUI, PAM module, and GNOME Shell extension.

The interface is defined in `gaze_core/src/dbus.rs` and exposed by the daemon via `zbus`.
