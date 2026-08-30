<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Configuration

Gaze is configured with `/etc/gaze/config.toml`.

Most users only need to change camera source or security level.

::: tip Editing config requires admin privileges
Settings are written through the daemon, which refuses unauthorized writes.
`gaze config` re-runs itself through `sudo` and prompts for your password;
`gaze config --show` is read-only and needs no privileges. The GUI settings
window instead authorizes through PolicyKit and uses the desktop's password
dialog, so make sure the `polkit` package is installed.
:::

## Default config

```toml
[inference]
execution_provider = "cpu"
device = "cpu"

[security]
level = "medium"

[cameras]
rgb = "primary"
# ir = "/dev/video2"        # optional infrared camera (direct /dev/video* node or usb:VVVV:PPPP)
# emitter_enabled = false   # drive the IR emitter (requires ir)
# parallel_capture = "never" # "never", "auto", or "always" (requires ir)
dark_luma_threshold = 20

[auth]
abort_if_ssh = true
abort_if_lid_closed = true
abort_before_first_resume = false
require_confirmation_lock_screen = false
require_confirmation_elevation = false
resume_grace_ms = 0
start_delay_ms = 0
start_delay_scope = "screen_lock"

[enrollment]
max_templates = 2
min_face_size_ratio = 0.25

[liveness]
enabled = true
threshold = 0.8
max_frames = 40

[storage]
encrypt_templates = false
```

## Upgrades

Package upgrades never overwrite an edited `/etc/gaze/config.toml`. If the
packaged default changed, the new template is saved alongside it as
`config.toml.rpmnew` (RPM) or `config.toml.pacnew` (Arch); on Debian/Ubuntu,
dpkg keeps your file and asks before replacing it. Any option missing from
your config uses its built-in default, so you don't need to merge new options
after upgrading.

## Select the inference device

Gaze always loads its `.onnx` models through ONNX Runtime.

The default uses the ONNX Runtime CPU execution provider:

```toml
[inference]
execution_provider = "cpu"
device = "cpu"
```

OpenVINO does not select a fixed device at build time. The same
OpenVINO-enabled Gaze binary can use an Intel CPU, GPU, or NPU. Select the
device in `/etc/gaze/config.toml`.

An installation with OpenVINO support should select the Intel NPU by default:

```toml
[inference]
execution_provider = "openvino"
device = "npu"
```

Change `device` to `"gpu"` to use the Intel GPU. This does not require
recompiling Gaze.

The Gaze daemon must also be compiled with the `openvino` Cargo feature. On a
CPU-only build, `gaze config` and the GUI refuse to set
`execution_provider = "openvino"`. A config file that already contains it does
not stop the daemon: it logs a warning and runs on the CPU, the same way every
other unusable value in `/etc/gaze/config.toml` is handled.

The values stay lowercase in the config. Gaze converts the device name only
when it calls OpenVINO. ONNX Runtime keeps its CPU execution provider after
OpenVINO. It runs unsupported model operations on the CPU. If Gaze cannot
create an OpenVINO session, it logs the error and creates a CPU session.
`gaze doctor --benchmark` reports the execution provider and device each model
actually uses, and warns when that is not the configured one.

## Change security level

`level` (under `[security]`) controls model choice and match strictness.

| Level | Detector | Recognizer | RGB / IR Threshold | Hybrid Policy | Notes |
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
rgb_threshold = 0.55
ir_threshold = 0.45
hybrid_policy = "or"    # optional; default, or, fallback_on_dark, and
```

RGB and IR similarity thresholds are independent for the custom level. The legacy `threshold` key remains accepted and supplies both values when the spectrum-specific keys are absent.

### Hybrid combining policy

`hybrid_policy` (under `[security]`, only configurable when `level = "custom"`) controls how RGB and IR (infrared) authentication results are combined when templates are enrolled for both modes and both cameras are available.

Supported policies:
- `or`: auth succeeds if either RGB or IR matches.
- `fallback_on_dark`: requires both, unless RGB is too dark (below `dark_luma_threshold`), in which case only IR is required.
- `and`: auth succeeds only if both RGB and IR match.
- `default`: a synonym for `fallback_on_dark`. It does not resolve to the policy
  the table above lists for the active level.

To get the policy the table lists for a level, omit `hybrid_policy` entirely.
The key is only read when `level = "custom"`, and a custom level with the key
absent also resolves to `fallback_on_dark`.

Both `fallback_on_dark` and `default` require RGB and IR to match when RGB was
never attempted at all, for example when the RGB camera could not be opened.
Only a frame that was captured and measured as too dark relaxes the requirement
to IR alone.

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

`pipewiresrc` needs a PipeWire session to attach to. GDM's greeter runs its own
user session and provides one, but greeters like KDE's `plasmalogin`, SDDM,
greetd, and a plain TTY do not. When the PipeWire source fails to open, Gaze logs
`Opening the PipeWire camera failed` and falls back to the first matching V4L2
node on its own, so `primary` still works in those greeters.

Pinning `rgb` to the camera directly skips that fallback and uses `v4l2src`
straight away. Prefer it when the machine has several cameras and you want a
specific one, rather than as a workaround for a greeter without a session:

```toml
[cameras]
rgb = "usb:046d:085e"   # resolve the color node for this USB VID:PID
# rgb = "/dev/video0"    # or a fixed V4L2 node
```

`usb:VVVV:PPPP` (hex VID:PID) resolves to whatever `/dev/video*` node that
camera exposes right now, picking the color node when a single-function webcam
presents both a color and an IR node under the same id. Prefer it over a raw
`/dev/video*` path, which silently points at the wrong device if the cameras
get renumbered.

### Dark-frame rejection

Gaze rejects frames that are too dark before running face detection:

```toml
[cameras]
dark_luma_threshold = 20
```

With the default, a frame is skipped when its mean luminance (0-255, BT.601 weighted) falls below 20. Raise it to reject dimmer scenes, lower it to be more permissive.

## Infrared (IR) camera

Gaze supports Windows Hello-style infrared (IR) cameras to enable multi-camera hybrid authentication. The `ir` setting may point directly to the IR camera's `/dev/video*` node:

```toml
[cameras]
ir = "/dev/video2"
emitter_enabled = false
```

You can also resolve the node by USB VID:PID (here it picks the mono/IR node),
or use an IR PipeWire/GStreamer source:

```toml
[cameras]
ir = "usb:046d:085e"
# ir = "pipewiresrc target-object=<pipewire-target>"
```

When `ir` is configured, Gaze captures from both the RGB and IR cameras. During enrollment, both cameras capture templates. During verification, Gaze combines the results according to the configured `hybrid_policy`.

### Parallel RGB + IR capture

By default, verification captures the two cameras one at a time (RGB, then IR). Capturing sequentially rather than concurrently lets single-function webcams that cannot stream their RGB and IR sensors at once (for example the Logitech BRIO 4K, `046d:085e`) still use hybrid authentication, at the cost of latency: with `hybrid_policy = "and"` both spectra always run, so the two capture phases add up.

If your camera can stream both sensors at once, `parallel_capture` restores the concurrent behavior:

```toml
[cameras]
ir = "/dev/video2"
parallel_capture = "auto"
```

| Value | Behavior |
| --- | --- |
| `never` (default) | Always capture RGB, then IR. Works on every camera. |
| `auto` | Capture in parallel only when RGB and IR are separate hardware functions. |
| `always` | Always capture in parallel. Use only if you know your camera supports it. |

`auto` resolves each configured source to its `/dev/video*` node and compares the hardware function behind it (for USB cameras, the sysfs USB interface the node hangs off). Two nodes on the same function are substreams of one device that only streams one mode at a time, so they stay serial even though their node numbers differ. This is the BRIO case, where `/dev/video0` and `/dev/video2` share a single UVC function.

The default `rgb = "primary"` names no node of its own: it means "whatever camera PipeWire hands out". Rather than guess which one that is, `auto` asks the question from the IR side: does the IR camera's own function also expose a colour node? If it does, the IR camera is a dual-sensor device that `primary` may well resolve to, so capture stays serial. If the IR function is infrared-only, it cannot be whatever `primary` turns out to be, and the two stream at once. A hand-written GStreamer pipeline in `rgb` is treated the same way. If nothing can be enumerated at all, `auto` keeps the serial path.

Parallel capture only changes *when* each spectrum is captured, never whether both have to pass. `hybrid_policy` behaves identically in both modes. The speedup is also bounded by face detection, which both spectra share, so expect a real improvement rather than a halving.

Some Windows Hello webcams expose their RGB and IR sensors as a single USB Video Class function and can't stream both at once. Enrollment still works since it only needs short bursts, but parallel RGB+IR verification drops the IR stream mid-loop (`IR camera stream stopped unexpectedly`) and auth falls back to password. If you hit this after setting `parallel_capture`, set it back to `never`. Enrollment always captures one camera at a time regardless of this setting.

The Logitech BRIO 4K (`046d:085e`) is a known example. That's the original BRIO, not the newer Brio 300/500/100, which use different product IDs.

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
abort_before_first_resume = false
require_confirmation_lock_screen = false
require_confirmation_elevation = false
resume_grace_ms = 0
start_delay_ms = 0
start_delay_scope = "screen_lock"
```

`abort_if_ssh` detects SSH sessions from the DBus caller process environment. `abort_if_lid_closed` reads ACPI lid state when available and is ignored on systems without a lid sensor.

`abort_before_first_resume` refuses face authentication until the machine has suspended and woken at least once, so the first authentication of a boot always falls through to the password. On GNOME that password is what unlocks the login keyring; authenticating with your face instead leaves the keyring locked and GNOME asks for the password again a moment later. With this enabled you type the password once at the GDM login and then unlock with your face for the rest of the session.

Gaze arms the gate from logind's `PrepareForSleep` signal, the same signal `resume_grace_ms` uses, so hibernation counts as well as suspend. The state lives in `gazed` and is not persisted: if the daemon restarts, the next authentication is blocked again until the next resume. It applies to every surface, including `gaze auth` and the GUI's test button, so a test right after boot will report a failure until you suspend once.

`require_confirmation_lock_screen` and `require_confirmation_elevation` each add a manual intent check step after a successful face match, and can be toggled independently. `require_confirmation_lock_screen` covers the lock screen and login/greeter screens (e.g. GDM); `require_confirmation_elevation` covers elevated auth prompts (`sudo`, `su`, `doas`, `run0`, polkit, `pkexec`). Both PAM modules honor them.

When confirmation is disabled, a successful match replaces the camera prompt with "Face Verified." and authentication continues immediately without waiting for input.

With the standard `pam-gaze` module (e.g. `sudo`, `gdm-face`):
- In a text-based (TTY) environment such as `sudo` in a terminal, it asks for text confirmation after the face match ("Press Enter to confirm, Esc to cancel").
- On the GNOME lock screen and GDM login screen (with the Gaze Extension active), it shows "Face Verified. Press Enter to confirm." below the password field; press Enter with the field empty to confirm. If the extension is inactive, the login is denied, because the extension is the expected confirmation channel on GNOME and Gaze will not silently skip the confirmation you asked for.
- In other graphical prompts without a TTY (e.g. the KDE lock screen, `hyprlock`), there is no channel that could answer the prompt, so the face match unlocks on its own. On the KDE lock screen in particular, asking would not reach anybody: the greeter never delivers a response to its biometric slot, so the request would hang that slot for the rest of the lock. If you want the confirmation step enforced on a surface that can show a dialog, use the `pam-gaze-grosshack` module.
- A **login greeter** is the exception: it never bypasses. GDM always runs GNOME with the Gaze Extension, so confirmation is enforced there or the login is denied.

A "text-based (TTY) environment" means Gaze can open the process's controlling terminal (`/dev/tty`), which is how `sudo` itself finds the terminal to prompt on. Redirected standard input does not change that, so `echo 1 | sudo tee /tmp/1` still confirms from the keyboard. When there is no controlling terminal at all (a management console such as Cockpit that drives PAM over a framed stdio protocol, or a service started without one), nobody can press a key, so Gaze neither prints a terminal banner nor waits for one; the face match is refused and the stack falls through to the password.

Callers that set the PAM `PAM_SILENT` flag, `sudo` among them, receive no messages through their own conversation. Gaze still writes the camera prompt and the verdict to the controlling terminal when there is one, so a terminal user keeps the "Please look at the camera" / "Face Verified." feedback; graphical callers with no terminal stay silent.

With the `pam-gaze-grosshack` module:
- The password prompt still comes up immediately so you are never blocked.
- If face verification succeeds before you finish entering your password:
  - In a text-based (TTY) environment, it cancels the password prompt and asks for text confirmation ("Press Enter to confirm, Esc to cancel").
  - In a graphical Polkit environment:
    - On **GNOME** (with the Gaze Extension active), it hides the password field, focuses the "Authenticate" button, and lets you confirm by pressing Enter or clicking the button. If the extension is inactive, it bypasses confirmation entirely to avoid locking you out.
    - On **KDE Plasma & LXQt**, it prompts you to press "OK" to confirm.
    - On **Hyprland**, it prompts you to press "Authenticate" to confirm.
    - On other graphical environments, it prompts you to press "Enter" to confirm.

`resume_grace_ms` delays face verification on system resume by the specified number of milliseconds (e.g. `3000` ms) to allow slower displays/GPUs to initialize and repaint, preventing verification from occurring behind a blank screen. Set to `0` to disable the delay.

`start_delay_ms` delays face verification by the specified number of milliseconds, not only after suspend. Set to `0` to disable the delay.

Use it when your lock screen unlocks itself the moment you lock it manually. Lockers differ in when they start authenticating: hyprlock starts its PAM stack as soon as it launches, and KDE's lock screen starts as soon as its UI appears, so if you are still sitting in front of the camera when you lock, Gaze matches your face and unlocks again immediately. A delay of `3000`-`5000` ms gives you time to step away. The GNOME lock screen does not need this, because face authentication there only begins once you dismiss the lock shield.

The delay is measured from when the session locked, not from each attempt, so a
second try during the same lock does not wait all over again. Gaze learns the lock
time from logind's `LockedHint`, which GNOME, KScreenLocker and hyprlock all set.
Where nothing sets it, every attempt waits the full delay.

`start_delay_scope` controls which prompts wait:

| Value | Effect |
| --- | --- |
| `all` | Every face authentication prompt waits, `sudo` and polkit prompts included. |
| `screen_lock` (default) | Only screen lockers wait. `sudo`, `su`, `doas`, `run0`, polkit, `pkexec` and display-manager greeters start scanning immediately. |

Neither scope delays `gaze auth` or the GUI's test button. Those call the daemon directly, with no PAM prompt and no locker to step away from, so they always start scanning at once. The `resume_grace_ms` wait still applies to them, because that one is about the camera and display settling after suspend.

The default `screen_lock` scope gives you the delay on your lock screen without a slow `sudo`:

```toml
[auth]
start_delay_ms = 3000
start_delay_scope = "screen_lock"
```

Gaze tells these apart by the PAM service name of the prompt, which is the only signal that works everywhere. On GNOME, for example, the same process drives both the lock screen and polkit dialogs, so nothing about the caller itself distinguishes them. A service Gaze does not recognize counts as a screen lock, so an unusual locker keeps the delay you configured rather than silently losing it. `gdm-face` counts as a screen lock by name, because GDM uses it for both the greeter and the lock screen; when the active session is a greeter the daemon reclassifies it as a login so the greeter does not wait.

To see what your prompts report, watch the daemon while you trigger one:

```bash
journalctl -u gazed -f | grep 'Face auth requested'
```

Four things to keep in mind:

- The scope is only honored by a daemon new enough to know about it. Package upgrades restart `gazed` if it was already running, but if you built or installed Gaze some other way and the old daemon is still live, the delay keeps applying to every prompt. Run `sudo systemctl restart gazed` if `screen_lock` appears to be ignored.
- On resume from suspend, Gaze waits for whichever of `start_delay_ms` and `resume_grace_ms` is longer. The two do not stack.
- `resume_grace_ms` ignores `start_delay_scope`. It exists so the display can repaint after suspend, which has nothing to do with which prompt is asking, so it still applies to the first authentication after a resume whatever that prompt is.
- With a sequential PAM stack (`hyprlock-gaze`, the default), `pam_gaze.so` runs before the password module, so the delay also postpones the point at which a typed password is accepted. You can type during the delay, but your first Enter may be consumed while PAM is still inside Gaze, requiring a second press. This is the same behavior as the existing wait while a face scan is in progress. The simultaneous stack (`hyprlock-gaze-simultaneous`) prompts for the password in parallel and avoids it.

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
min_face_size_ratio = 0.25
```

Increase `max_templates` if auth is unreliable in varied lighting.

`min_face_size_ratio` controls the smallest detected face accepted during enrollment,
as a fraction of the frame's shorter side. The default `0.25` requires the face to
occupy at least one quarter of that dimension. Lowering it permits enrollment from
farther away; for example, `0.20` permits a face roughly 25% farther away than the
default. Values from `0.10` through `0.75` are accepted.

This is an enrollment-quality gate only; authentication does not impose the same
centering and proximity threshold. Use the highest value that remains comfortable,
because smaller face crops contain less detail for the enrolled template.

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

Alongside the model, Gaze watches how far your eyes travel between frames, measured against the distance between them so it does not depend on how close you sit. A run that has accumulated several frame pairs and never seen movement above that floor is treated as a still object and refused even when the model is confident. Moving normally (breathing, blinking, small head shifts) clears it; once any pair shows movement, holding still afterwards does not undo it. On IR cameras this movement check is the whole liveness test, since the anti-spoof model is trained on colour frames.

`max_frames` caps how many valid face frames Gaze examines before giving up and falling back to your password. It bounds the whole attempt, not just the liveness stage: an unrecognised face spends the same budget. Frames only count while a usable face is in view, and the RGB and IR phases each get the full budget, so 40 frames is roughly one to two seconds of looking at the camera on a typical 30fps webcam. Raise it if authentication gives up before you are ready; `gaze auth --verbose` reports when a run ends this way.

## Recommended tuning workflow

1. Start with `[security] level = "medium"`
2. Enroll one profile: `gaze add-face default`
3. Test 5 to 10 times using `gaze auth --verbose`
4. If photo or screen spoofing is a concern, keep `[liveness] enabled = true`
5. If false accepts are too high, switch to `high`
6. If false rejects are too high, run `gaze refine-face default`
