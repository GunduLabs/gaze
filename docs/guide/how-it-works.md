# How Gaze Works

This page explains the internals of Gaze's facial authentication pipeline. You don't need it to use Gaze, but it helps understand why it behaves the way it does.

## Security warning

Gaze is currently **not suitable for security-critical authentication**.

Its liveness model raises the bar for printed-photo and screen-photo attacks, but it should not be your only authentication factor. Video replay, high-quality presentation attacks, and missing or disabled liveness checks remain risks.

Gaze supports infrared (IR) cameras: configure `cameras.ir` with a GStreamer/PipeWire source string and Gaze captures through it, optionally driving the camera's IR emitter during authentication. See the configuration guide for setup. Further anti-spoofing protections are planned for upcoming releases.

## Privacy model

- Face processing runs locally on your machine.
- No cloud account is required.
- Face embeddings are stored on disk under your local Gaze data path.

## Authentication pipeline

```text
Camera frame -> Face detection (SCRFD) -> Face alignment -> Embedding (ArcFace/ResNet50) -> Similarity match -> Liveness check (MiniFASNet-V2 / eye-motion on IR)
```

High level:

1. Camera frame is captured from your configured GStreamer camera source.
2. Detector finds a face and facial landmarks.
3. Face is aligned into a standard input shape.
4. Recognition model creates an embedding vector.
5. Embedding is compared against your enrolled profiles. When both RGB and IR cameras are active, authentication results are combined based on the configured hybrid combining policy (e.g. requiring both to match, either to match, or dynamically falling back to IR in dark scenes).
6. If liveness is enabled, a MiniFASNet-V2 anti-spoofing model checks the detected face crop (on the IR camera path, an eye-motion check across frames is used instead).

If best similarity passes threshold and the liveness score passes threshold, auth succeeds.

## Why multiple captures help

Each enrollment stores multiple samples across slightly different angles.

That makes authentication more robust for:

- Small head rotation
- Minor lighting changes
- Appearance shifts (for example, glasses)

## Where data is stored

Default locations:

- User embeddings: `/var/lib/gaze/users`
- Model files: `/var/cache/gaze`
- Config file: `/etc/gaze/config.toml`

## Components

- `gazed`: daemon that performs detection and recognition (crate: `gaze`)
- `gaze`: CLI client (crate: `gaze-cli`, kept separate so the client binary does not link ONNX Runtime)
- `gaze-gui`: GTK app
- PAM integration and GNOME extension for login/lock screen flow

The CLI and GUI communicate with daemon over DBus (`com.gundulabs.Gaze`).
