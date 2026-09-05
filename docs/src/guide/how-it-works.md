<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

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
Camera frame -> Face detection (SCRFD) -> Face alignment -> Embedding (ArcFace: MobileFaceNet by default, ResNet50 at high/maximum security) -> Similarity match -> Liveness check (MiniFASNet-V2 / eye-motion on IR)
```

High level:

1. Camera frame is captured from your configured GStreamer camera source.
2. Detector finds a face and facial landmarks.
3. Face is aligned into a standard input shape.
4. Recognition model creates an embedding vector.
5. Embedding is compared against your enrolled profiles. When both RGB and IR cameras are active, authentication results are combined based on the configured hybrid combining policy (e.g. requiring both to match, either to match, or dynamically falling back to IR in dark scenes).
6. If liveness is enabled, a MiniFASNet-V2 anti-spoofing model checks the detected face crop (on the IR camera path, an eye-motion check across frames is used instead).

If best similarity passes threshold and the liveness score passes threshold, auth succeeds.

## One authentication attempt

Everything below happens once per `gaze auth`, PAM prompt, or lock screen unlock. The
same daemon code runs on every surface, so a problem you can reproduce with
`gaze auth --verbose` is the same problem the lock screen hits.

### Before the camera opens

1. The client claims the daemon over DBus. A claim binds the run to one user's PipeWire
   session, and the client releases it when it finishes. A claim that is never released is
   reclaimed after 5 minutes so the camera cannot be held hostage.
2. Gaze checks three refusals in order: no suspend or resume since boot
   (`auth.abort_before_first_resume`), the caller sits inside an SSH session
   (`auth.abort_if_ssh`), and the laptop lid is closed (`auth.abort_if_lid_closed`). Any of
   them ends the attempt before a camera is touched.
3. Gaze works out which spectra can run. RGB runs when `cameras.rgb` is set *and* you have
   RGB templates enrolled; IR runs on the same rule. If neither can run, or if
   `security.hybrid_policy = "and"` demands both but you enrolled only one, the attempt ends
   here rather than quietly authenticating on half the evidence.
4. `auth.start_delay_ms` and `auth.resume_grace_ms` hold the capture back. Time the screen has
   already been locked counts against the delay, so a screen locked longer than
   `start_delay_ms` starts immediately. The lid is checked once more after the delay, since it
   can close while Gaze waits.

### Opening the camera

5. The configured source becomes a GStreamer pipeline. `primary` becomes a bare `pipewiresrc`,
   a PipeWire target becomes `pipewiresrc target-object=...`, and a `/dev/videoN` path or USB
   `vid:pid` becomes `v4l2src`. Under a claim, `pipewiresrc` is bound to the claiming user's
   session socket rather than resolving one from the environment.
6. The pipeline gets 500 ms to reach the playing state. A slower camera is not treated as a
   failure: the frame loop reports any error that arrives later.
7. If a PipeWire source fails to open at all, Gaze retries once through that camera's own V4L2
   node. A pinned target retries only the node behind that same camera; `primary` may retry any
   color camera it can reach.

### Reading frames

8. **Warm-up.** Sensors that run their own auto-exposure stream black frames for a moment after
   the pipeline starts, so an early dark frame says nothing about the room. For up to 2 seconds
   those frames are ignored instead of reported. The grace is conditional: if the stream has not
   brightened at all after 1 second and at least 8 frames, Gaze concludes nothing is warming up
   and reports the darkness immediately. The grace is also spent the moment the stream is ever
   lit, so a lens covered mid-scan is reported at once. The warm-up is skipped entirely when a
   dark RGB frame would hand the camera straight to IR, so hybrid setups keep their immediate
   hand-off.
9. **Dark gate.** A frame whose mean luma is below `cameras.dark_luma_threshold` (default 20)
   is `TooDark` and never reaches the detector. If a PipeWire stream is still dark when its
   warm-up expires, Gaze reopens the same camera once on its V4L2 node and gives the new stream
   its own warm-up, which is how a camera that works through `/dev/videoN` but not through
   PipeWire recovers on its own.
10. **Detection and matching.** The detector finds a face and landmarks, the face is aligned,
    the recognition model produces an embedding, and the embedding is compared against your
    enrolled templates. An embedding is only computed for a `Usable` frame, so a frame that is
    dark or badly framed costs a detection and nothing more.
11. **Liveness.** On a match, the anti-spoof model scores the face crop, and an eye-motion check
    runs across frames. Both must pass. A face that matches but fails liveness reports
    `Face matched, but the liveness check did not pass`.
12. **IR.** By default (`cameras.parallel_capture = "never"`) the IR phase waits for RGB to
    release the camera, then engages the emitter if one is configured. Windows Hello emitters
    strobe, so the IR path tolerates a run of dark frames and only reports darkness after 8
    consecutive ones.

### Deciding

The result is a verdict plus the last capture status from each spectrum. When both spectra
ran, `security.hybrid_policy` decides how they combine. The verdict travels back to the client
over DBus, and PAM turns it into a message and a return code.

## Timeouts and budgets

A watchdog checks the deadlines every 250 ms, so a camera that stops delivering frames
entirely is still bounded.

| Deadline | Value | Measured from | What happens |
| --- | --- | --- | --- |
| Start delay | `auth.start_delay_ms`, at least `auth.resume_grace_ms` after a resume (both default 0) | the request | Capture is held back. Cancellable. |
| Pipeline start | 500 ms | `Playing` requested | Gaze stops waiting for the state change, but keeps the pipeline. |
| Camera warm-up | up to 2 s, cut short after 1 s and 8 frames if the stream never brightens | the first frame | Dark frames are ignored rather than reported. |
| Dark stream | 1 s | the first `TooDark` after warm-up | Attempt ends. "Need more light". |
| No face | 5 s | the last frame with a face in it | Attempt ends. "Face not detected". |
| No usable frame | 8 s | the last frame that produced an embedding | Attempt ends. Bounds a face that stays badly framed. |
| Serial RGB budget | 4 s | the RGB phase start | RGB hands the camera to IR even without a match. Only when IR also runs and `parallel_capture = "never"`. |
| Frame budget | `liveness.max_seconds` x camera fps, minimum 6 frames (default 2.0 s, so 60 frames at 30 fps) | frames counted only while a usable face is in view, per spectrum | Attempt ends. Logged as `frame budget spent`. |
| PAM backstop | 12 s plus the start delay | the PAM prompt | PAM stops waiting. "Face authentication timed out". |
| Client backstop | 20 s | the DBus call | The CLI or GUI stops waiting on the daemon. |
| Claim | 5 min | the claim | An unreleased claim is reclaimed. |

The deadlines nest deliberately: every daemon deadline fires before the PAM backstop, and the
PAM backstop fires before the client one, so you always get the specific reason rather than a
generic timeout.

## What the camera reports while it runs

Each frame produces one status. The GUI and the CLI show its text, and the daemon emits it over
DBus as `FaceStatus`.

| Status | Shown as | Means |
| --- | --- | --- |
| `Unused` | Camera is not in use... | That spectrum did not run. |
| `NoFace` | Please look at the camera... | The detector found nothing. |
| `TooDark` | Need more light... | The frame, or the face region in it, is below `cameras.dark_luma_threshold`. |
| `Clipped` | Face is clipped. Please move back... | The face touches the edge of the frame. |
| `Ready` | Hold still... | A face was found and this frame is dark, but the rolling average over the last 5 frames is not. A transient dip, not a verdict. |
| `Usable` | Hold still... | A face was found and is bright enough to match. Only these frames produce an embedding. |

`NotCentered`, `TooFar`, and `TooClose` exist too, but only enrollment checks centering and
proximity. During authentication you will never see them.

`Ready` and `Clipped` count as "a face is in view", so they hold off the no-face deadline but
not the no-usable one. That is why a face that is present but never quite usable ends at 8
seconds rather than running forever.

## How an attempt ends

| Ending | CLI | PAM message | PAM code |
| --- | --- | --- | --- |
| Match | `Authenticated as: <face>` | Face Verified. | success |
| A face was judged and did not match | `Authentication failed` | Face not recognized. Enter your password. | `PAM_AUTH_ERR` |
| Matched, but liveness did not pass | `Authentication failed` plus the liveness note | Face not recognized. Enter your password. | `PAM_AUTH_ERR` |
| Ended too dark | `Authentication failed` | Too dark for face authentication. Enter your password. | `PAM_AUTHINFO_UNAVAIL` |
| Ended with no face | `Authentication failed` | Face not detected. Enter your password. | `PAM_AUTHINFO_UNAVAIL` |
| The camera never ran, or the daemon could not be reached | `Authentication failed` | Face authentication unavailable. Enter your password. | `PAM_AUTHINFO_UNAVAIL` |
| PAM budget spent | n/a, PAM only | Face authentication timed out. Enter your password. | `PAM_AUTHINFO_UNAVAIL` |

Only a face that was actually judged returns `PAM_AUTH_ERR`. Every other ending returns
`PAM_AUTHINFO_UNAVAIL`, which tells PAM that Gaze reached no decision rather than that you
failed an attempt, so a dark room or a camera in use by another program does not count against
you. A run that ends on a hardware error is reported the same way.

## Reading it in the logs

`journalctl -u gazed -b` records the shape of every attempt. The lines that map to the
deadlines above:

| Log line | Meaning |
| --- | --- |
| `VerifyStart: sensing faces for user ...` | Capture is starting. Names the spectra, the liveness settings, and whether capture is serial. |
| `Attempting to open GStreamer camera: ...` | The exact pipeline. Useful for confirming which source was actually used. |
| `Opening the PipeWire camera failed ...; trying a direct V4L2 device` | The PipeWire source did not open, and the V4L2 retry is next. |
| `RGB stream stayed dark through PipeWire ...; retrying on /dev/videoN` | The stream opened but never brightened, so the same camera is being reopened on its own node. |
| `RGB stream never brightened: mean_luma=0` | The stream is flat dark. A closed privacy shutter, a lens cap, or a genuinely dark room. |
| `RGB face region: mean_luma=..., threshold=..., status=...` | The luma reading behind a status. The first frame of each distinct status is logged. |
| `VerifyStart: giving up after 1000ms of dark frames` | The dark deadline. |
| `VerifyStart: giving up after 5s without a detected face` | The no-face deadline. |
| `VerifyStart: giving up after 8s without a usable frame` | The no-usable deadline. |
| `VerifyStart: frame budget spent` | The frame budget. `matched=true` on the same line means the face matched but liveness did not pass. |

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
- PAM integration, the GNOME extension, and KDE's biometric PAM slot for login/lock screen flow

The CLI and GUI communicate with daemon over DBus (`com.gundulabs.Gaze`).
