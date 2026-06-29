# How Gaze Works

This page explains the internals of Gaze's facial authentication pipeline. You don't need it to use Gaze, but it helps understand why it behaves the way it does.

## Security & Liveness

Gaze provides facial authentication with local liveness anti-spoofing and support for infrared (IR) cameras.

When using an IR camera and RGB liveness checking, Gaze offers significant resistance against presentation attacks, such as printed photos, screen replays, or video-based spoofing. For high-security environments, it is recommended to keep standard system authentication (such as password entry) configured as a backup or fallback factor.

## Privacy model

- Face processing runs locally on your machine.
- No cloud account is required.
- Face embeddings are stored on disk under your local Gaze data path, readable only by root.
- They can optionally be encrypted at rest with a key sealed to the TPM, so a stolen disk is useless on another machine. See [template encryption](/guide/configuration#encrypt-face-templates-with-the-tpm).

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
- TPM-sealed encryption key (only when template encryption is enabled): `/var/lib/gaze/tpm`
- Model files: `/var/cache/gaze`
- Config file: `/etc/gaze/config.toml`

## Components

- `gazed`: daemon that performs detection and recognition (crate: `gaze`)
- `gaze`: CLI client (crate: `gaze-cli`, kept separate so the client binary does not link ONNX Runtime)
- `gaze-gui`: GTK app
- PAM integration and GNOME extension for login/lock screen flow

The CLI and GUI communicate with daemon over DBus (`com.gundulabs.Gaze`).
