# Configuration

Gaze is configured with `/etc/gaze/config.toml`.

Most users only need to change camera source or security level.

::: tip Editing config requires admin authorization
`gaze config` writes settings through the daemon, which is guarded by
PolicyKit, so you are prompted for an administrator password when saving. Over
SSH or on a plain TTY, `gaze` registers a terminal agent (`pkttyagent`) so the
prompt appears in your shell; make sure the `polkit` package is installed. In a
graphical session the desktop's password dialog is used instead.
:::

## Default config

```toml
[security]
level = "medium"

[cameras]
rgb = "primary"
# ir = "/dev/video2"        # optional infrared camera; direct nodes are allowed only for IR
# emitter_enabled = false   # drive the IR emitter (requires ir)
dark_luma_threshold = 30

[auth]
abort_if_ssh = true
abort_if_lid_closed = true
require_confirmation = false
resume_grace_ms = 0

[enrollment]
max_templates = 2

[liveness]
enabled = true
threshold = 0.8
max_frames = 40

[storage]
encrypt_templates = false
```

## Change security level

`level` (under `[security]`) controls model choice and match strictness.

| Level | Detector | Recognizer | Threshold | Hybrid Policy | Notes |
|---|---|---|---|---|---|
| `low` | SCRFD-500M | MobileFaceNet | 0.30 | `or` | Fastest |
| `medium` | SCRFD-500M | MobileFaceNet | 0.40 | `fallback_on_dark` | Default |
| `high` | SCRFD-10G | ResNet50 | 0.50 | `fallback_on_dark` | More accurate |
| `maximum` | SCRFD-10G | ResNet50 | 0.60 | `and` | Most strict |

Practical guidance:

- `medium`: best starting point for most laptops
- `high`: use when false positives are unacceptable
- `low`: use on weaker hardware when speed is critical

### Custom level

```toml
[security]
level = "custom"
detector = "accurate"   # "standard" or "accurate"
recognizer = "accurate" # "standard" or "accurate"
threshold = 0.55
hybrid_policy = "or"    # optional; default, or, fallback_on_dark, and
```

### Hybrid combining policy

`hybrid_policy` (under `[security]`, only configurable when `level = "custom"`) controls how RGB and IR (infrared) authentication results are combined when templates are enrolled for both modes and both cameras are available.

Supported policies:
- `default`: uses the policy shown in the table above for the active level.
- `or`: auth succeeds if either RGB or IR matches.
- `fallback_on_dark`: requires both, unless RGB is too dark (below `dark_luma_threshold`), in which case only IR is required.
- `and`: auth succeeds only if both RGB and IR match.

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
dark_luma_threshold = 30
```

With the default, a frame is skipped when its mean luminance (0-255, BT.601 weighted) falls below 30. Raise it to reject dimmer scenes, lower it to be more permissive.

## Infrared (IR) camera

Gaze supports Windows Hello-style infrared (IR) cameras to enable multi-camera hybrid authentication. Unlike the RGB setting, `ir` may point directly to the IR camera's `/dev/video*` node:

```toml
[cameras]
ir = "/dev/video2"
emitter_enabled = false
```

You can also use an IR PipeWire/GStreamer source:

```toml
[cameras]
ir = "pipewiresrc target-object=<pipewire-target>"
```

When `ir` is configured, Gaze captures from both the RGB and IR cameras. During enrollment, both cameras will capture templates, and during verification, they will authenticate in parallel, combining results according to the configured `hybrid_policy`.

### IR emitter blaster

Many IR cameras automatically light their infrared LED when streaming starts. If yours does not, set `emitter_enabled = true` to manually drive the emitter during authentication.

Gaze resolves the underlying `/dev/video*` node from the PipeWire camera, matches it by USB VID:PID against a small built-in table, and also probes at runtime for the standard Microsoft Face Authentication control to send UVC toggle requests. If the emitter does not light even with `emitter_enabled = true`, the camera may need a profile added under `gaze-core/ir-profiles/`.

On the IR path, liveness uses eye-motion analysis across frames; the RGB MiniFASNet model is not applied to infrared.

Driving the emitter blaster needs read/write access to the IR `/dev/video*` node. The daemon runs as root and is a member of the `video` group, so the default `root:video` device permissions are sufficient; no extra udev rule is required.

## Authentication options

Gaze skips face authentication in sessions where the camera is unlikely or unsafe to use:

```toml
[auth]
abort_if_ssh = true
abort_if_lid_closed = true
require_confirmation = false
resume_grace_ms = 0
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

`resume_grace_ms` delays face verification on system resume by the specified number of milliseconds (e.g. `3000` ms) to allow slower displays/GPUs to initialize and repaint, preventing verification from occurring behind a blank screen. Set to `0` to disable the delay.

After changing config:

```bash
sudo systemctl restart gazed
```

## Storage paths

Storage locations are managed by the service setup and are not intended to be changed in config:

- User embeddings: `/var/lib/gaze/users`
- Downloaded models: `/var/cache/gaze`

Models are auto-downloaded on first run if missing.

## Encrypt face templates with the TPM

By default, enrolled face embeddings are stored as plaintext files under
`/var/lib/gaze/users` (readable only by root). On a machine with a TPM 2.0 chip
you can additionally encrypt them at rest:

```toml
[storage]
encrypt_templates = true
```

When enabled, `gazed` seals a random AES-256 key to the TPM and stores every
embedding AES-256-GCM encrypted under it. The sealed key lives in
`/var/lib/gaze/tpm` and can only be unsealed by **this** TPM, so a stolen disk
(or a backup restored on another machine) yields nothing usable.

Behavior to be aware of:

- **Fail-closed.** If `encrypt_templates = true` but no usable TPM is found, the
  daemon refuses to start rather than silently writing unprotected biometrics.
  Check `journalctl -u gazed` and either fix the TPM (e.g. enable it in firmware)
  or set the flag back to `false`.
- **Machine binding only.** The key is sealed to the TPM's storage hierarchy
  with no PCR policy, so firmware, kernel, and Secure Boot updates do **not**
  lock you out. It protects against the disk leaving the machine, not against
  boot-chain tampering on the machine itself.
- **Automatic migration.** Edit the flag in `/etc/gaze/config.toml` and restart
  the daemon. Turning it on encrypts any existing plaintext templates in place;
  turning it off decrypts them back to plaintext, which also needs the TPM that
  sealed them.
- **TPM reset.** If the TPM is cleared, the sealed key (and therefore the
  encrypted templates) becomes unrecoverable. Delete `/var/lib/gaze/tpm` and
  re-enroll. The daemon will not start with sealed data it can no longer unseal.

Apply changes with:

```bash
sudo systemctl restart gazed
```

## Enrollment behavior

```toml
[enrollment]
max_templates = 2
```

Increase this if auth is unreliable in varied lighting.

### Multi-Camera & Hybrid Enrollment

Gaze supports enrolling face profiles for both RGB and IR cameras. Depending on your camera configuration at the time of enrollment:

- **Single Camera Setup**: If only the RGB camera is configured (the default), Gaze will capture and save templates only for the RGB spectrum.
- **Dual Camera (Hybrid) Setup**: If both the RGB and IR cameras are configured, Gaze will capture from both cameras concurrently. Each enrollment step will wait for valid aligned frames from both sensors.

### Upgrading Existing Profiles

If you connect or configure an IR camera after you have already enrolled a face, your existing face profiles will only contain RGB captures. 
- You can see which capture types exist for each face profile in the CLI (`gaze list-faces`) and the GUI settings window, which display `[RGB]` and `[IR]` status badges.
- To add the missing IR captures to an existing profile, ensure your IR camera is configured, and run:
  ```bash
  gaze refine-face <profile-name>
  ```
  Or refine the profile using the GUI. Gaze will run the camera stream to capture the missing spectrum and merge the new templates into your existing profile.

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
