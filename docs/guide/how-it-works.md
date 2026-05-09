# How Gaze Works

This page explains the internals of Gaze's facial authentication pipeline. You don't need it to use Gaze, but it helps understand why it behaves the way it does.

## Security warning

Gaze is currently **not suitable for security-critical authentication**.

It can be spoofed with a simple photo of the enrolled user, including a photo displayed on a screen.

Liveness detection, IR camera support, and other anti-spoofing protections are planned for upcoming releases.

## Privacy model

- Face processing runs locally on your machine.
- No cloud account is required.
- Face embeddings are stored on disk under your local Gaze data path.

## Authentication pipeline

```text
Camera frame -> Face detection -> Face alignment -> Embedding -> Similarity match
```

High level:

1. Camera frame is captured from your configured GStreamer camera source.
2. Detector finds a face and facial landmarks.
3. Face is aligned into a standard input shape.
4. Recognition model creates an embedding vector.
5. Embedding is compared against your enrolled profiles.

If best similarity passes threshold, auth succeeds.

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

- `gazed`: daemon that performs detection and recognition
- `gaze`: CLI client
- `gaze-gui`: GTK app
- PAM integration and GNOME extension for login/lock screen flow

The CLI and GUI communicate with daemon over DBus (`com.gundulabs.Gaze`).
