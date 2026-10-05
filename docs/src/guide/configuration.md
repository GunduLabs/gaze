<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Configuration

Gaze is configured with `/etc/gaze/config.toml`.

The defaults work for many setups. Common adjustments include choosing a camera
source or changing the security level.

::: tip Editing config requires admin privileges
The daemon requires authorization before saving changes. `gaze config` prompts
for your password through `sudo`, while `gaze config --show` is read-only and
needs no privileges. The GUI asks for authorization through PolicyKit and uses
your desktop's password dialog, so make sure the `polkit` package is installed.
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
# ir_frame_width = 340      # force the IR resolution; set with ir_frame_height (requires ir)
# ir_frame_height = 340
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
max_seconds = 2.0

[storage]
encrypt_templates = false
unlock_kwallet = false # optional TPM-backed KDE wallet unlock
unlock_gnome_keyring = false

[duress]
enabled = false
closed_threshold = 0.9
hold_ms = 600
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

Standard builds also support Intel OpenVINO and AMD Ryzen AI. To use either one,
install its drivers and runtime as described in the
[hardware acceleration guide](/guide/acceleration), then select automatic NPU
acceleration:

```toml
[inference]
execution_provider = "auto"
device = "npu"
```

| Provider | Devices | Behavior |
| --- | --- | --- |
| `cpu` | `cpu` | Bundled ONNX Runtime CPU inference (default) |
| `auto` | `npu` | Select Intel (`intel_vpu`) or AMD (`amdxdna`) from the bound NPU driver |
| `openvino` | `cpu`, `gpu`, `npu` | Use the installed Intel OpenVINO runtime |
| `vitis` | `npu` | Use the installed AMD Ryzen AI/Vitis AI runtime |

Restart `gazed` after installing a runtime or changing provider vendors. ONNX Runtime
is loaded once per daemon process. Changing an OpenVINO device within that runtime
can still reload models without restarting.

Missing libraries, unavailable drivers, model compilation failures, or failed startup
warmups produce a logged reason and a CPU session for the affected model. CPU graph
partitions may remain inside a successful accelerator session too; the provider label
alone does not prove that every operation ran on the NPU. `gaze doctor --benchmark`
reports each model's selected provider, timings, and any session fallback reason.

OpenVINO's NPU support targets Intel. AMD uses `vitis`, with the hardware and OS
restrictions documented by AMD. Neither provider makes an ordinary CPU or GPU an NPU.

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

## Select a camera source

The default camera source is:

```toml
[cameras]
rgb = "primary"
```

`primary` selects the first color `/dev/video*` node. To use a specific PipeWire
camera, open `gaze config` or set `rgb` to a GStreamer source:

```toml
[cameras]
rgb = "pipewiresrc target-object=<pipewire-target>"
```

For authentication and enrollment, the privileged daemon captures the backing
kernel `/dev/video*` node directly with `v4l2src`; it never connects to a
user-session PipeWire socket. The authenticated user controls that socket and
every virtual camera it advertises, so trusting it could let injected frames
reach face authentication. Gaze resolves a pinned PipeWire target to its own
V4L2 node in the same way, and refuses sources without a kernel node, including
hand-written GStreamer pipelines. As a result, `primary` also works in greeters
and on plain TTYs without a PipeWire session.

Pinning `rgb` to the camera directly uses `v4l2src` straight away without
resolving a PipeWire target first. Prefer it when the machine has several
cameras and you want a specific one:

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

When you configure an IR camera alongside RGB, Gaze captures templates from both
during enrollment, then combines their results during verification according to
the configured `hybrid_policy`.

### IR frame size override

Some laptop IR cameras advertise several resolutions but only stream valid frames
at their native one. Gaze picks the largest mode it can negotiate, so on those
cameras the IR feed comes out solid green or corrupted. Force the native size
instead:

```toml
[cameras]
ir = "/dev/video2"
ir_frame_width = 340
ir_frame_height = 340
```

- Set both keys or neither. Each must be between 1 and 4096. Leave them out to
  let Gaze negotiate the size, which is the default.
- If the camera cannot open at the forced size, Gaze logs a warning and falls back
  to negotiating the size, so a wrong value never leaves face auth without a camera.
- The override applies only to the IR camera. On Dell and Realtek modules that
  Gaze forces into 640x480 YUY2 mode, it replaces that 640x480.
- In the GUI, turn on **IR Frame Size Override** under Hardware.
- The daemon must be at least as new as the GUI or CLI that sets the override.
  An older daemon rejects it and leaves the rest of the configuration unchanged.

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
| `always` | Always capture in parallel. Choose this only after confirming your camera supports simultaneous streaming. |

`auto` resolves each configured source to its `/dev/video*` node and compares the hardware function behind it (for USB cameras, the sysfs USB interface the node hangs off). Two nodes on the same function are substreams of one device that only streams one mode at a time, so they stay serial even though their node numbers differ. This is the BRIO case, where `/dev/video0` and `/dev/video2` share a single UVC function.

The default `rgb = "primary"` means the first color `/dev/video*` node. Rather than guess which one that is, `auto` asks the question from the IR side: does the IR camera's own function also expose a colour node? If it does, the IR camera is a dual-sensor device that `primary` may well resolve to, so capture stays serial. If the IR function is infrared-only, it cannot be whatever `primary` turns out to be, and the two stream at once. An unresolvable `rgb` value is treated the same way. If nothing can be enumerated at all, `auto` keeps the serial path.

Parallel capture only changes *when* each spectrum is captured, never whether both have to pass. `hybrid_policy` behaves identically in both modes. The speedup is also bounded by face detection, which both spectra share, so expect a real improvement rather than a halving.

Some Windows Hello webcams expose their RGB and IR sensors through one USB Video
Class function and cannot stream both at once. Enrollment still works because it
captures only short bursts, but parallel RGB+IR verification can drop the IR
stream mid-loop (`IR camera stream stopped unexpectedly`) and fall back to your
password. If this happens after you enable `parallel_capture`, switch it back to
`never`. Enrollment always captures from one camera at a time, regardless of
this setting.

The Logitech BRIO 4K (`046d:085e`) is a known example. That's the original BRIO, not the newer Brio 300/500/100, which use different product IDs.

### IR emitter blaster

Many IR cameras automatically light their infrared LED when streaming starts. If yours does not, set `emitter_enabled = true` to manually drive the emitter during authentication.

Gaze resolves the underlying `/dev/video*` node from the PipeWire camera, matches USB cameras by VID:PID against a small built-in table, and probes for the standard Microsoft Face Authentication UVC control. The files under `crates/gaze-core/ir-profiles/` describe USB UVC extension-unit requests only.

Non-USB emitters driven over I2C use the reviewed profiles in `crates/gaze-core/i2c-ir-profiles/`, which are compiled into Gaze and never read from user configuration. The only one today is the Surface Pro 4 OV7251 sensor. It needs:

- The `i2c-dev` kernel module loaded, so the sensor's `/dev/i2c-*` bus exists. To load it at every boot, run `echo i2c-dev | sudo tee /etc/modules-load.d/i2c-dev.conf`.
- A userspace bridge that relays the IPU3/CIO2 IR stream into a v4l2loopback device at `/dev/video42` named `Surface IR Camera`, and writes the path of the CIO2 source node to `/run/surface_ir_bridge_dev`. Point `cameras.ir` at `/dev/video42`.
- The `ov7251` driver bound to `i2c-INT347E:00`.

Gaze uses the I2C bus on the adapter where the sensor is bound. Before writing,
it checks that a driver has claimed the sensor's address; it then changes only
the emitter bit and reads the register back. `gaze doctor` reports which check
failed. This register value has only been verified with the Surface Pro 4 wiring,
so it may not work with other Surface models or OV7251 devices.

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

`abort_if_ssh` asks logind whether the D-Bus caller's session is remote, falling back to the caller process environment and ancestry where logind is unreachable. `abort_if_lid_closed` reads ACPI lid state when available and is ignored on systems without a lid sensor.

Aborting face authentication when you type a password is not a key in this file. It is a property of the PAM stack: [simultaneous mode](/guide/pam#what-gaze-installs) (`pam_gaze.so simultaneous`) stands Gaze down as soon as you submit a password, and [retry mode](/guide/pam#retry-after-a-rejected-password) (`pam_gaze.so retry`) gives face auth one more attempt if that password turns out to be wrong.

`abort_before_first_resume` refuses face authentication until the machine has suspended and woken at least once, so the first authentication of a boot always falls through to the password. On GNOME that password is what unlocks the login keyring; authenticating with your face instead leaves the keyring locked and GNOME asks for the password again a moment later. With this enabled you type the password once at the GDM login and then unlock with your face for the rest of the session.

Gaze arms the gate from logind's `PrepareForSleep` signal, the same signal `resume_grace_ms` uses, so hibernation counts as well as suspend. The state lives in `gazed` and is not persisted: if the daemon restarts, the next authentication is blocked again until the next resume. It applies to every surface, including `gaze auth` and the GUI's test button, so a test right after boot will report a failure until you suspend once.

`require_confirmation_lock_screen` and `require_confirmation_elevation` each add a manual intent check step after a successful face match, and can be toggled independently. `require_confirmation_lock_screen` covers the lock screen and login/greeter screens (e.g. GDM); `require_confirmation_elevation` covers elevated auth prompts (`sudo`, `su`, `doas`, `run0`, polkit, `pkexec`). Both PAM modules honor them.

When confirmation is disabled, a successful match replaces the camera prompt with "Face Verified." and authentication continues immediately without waiting for input.

With the standard sequential `pam_gaze` mode (e.g. `sudo`, `gdm-face`):
- In a text-based (TTY) environment such as `sudo` in a terminal, it asks for text confirmation after the face match ("Press Enter to confirm, Esc to cancel").
- On the GNOME lock screen, GDM login screen, and unified Cinnamon lock screen (with the Gaze Extension active), it shows "Face Verified. Press Enter to confirm." below the password field (or presents a dedicated "Confirm Face Unlock" button); press Enter or click the button to confirm. If the extension is inactive, the login is denied, because the extension is the expected confirmation channel on GNOME and Gaze will not silently skip the confirmation you asked for.
- Where there is no controlling terminal but the PAM conversation can still prompt (e.g. `hyprlock`), it asks "Face Verified. Type 'yes' to confirm." An empty answer never counts as consent there: hosts that answer unknown prompts with `""` would otherwise auto-confirm without the user doing anything.
- On the KDE lock screen biometric slots (`kde-fingerprint`, `kde-smartcard`) and the `plasmalogin-fingerprint` greeter helper, there is no channel that could answer at all, so the face match unlocks on its own and `require_confirmation_lock_screen` is silently ignored there by design. Asking would not reach anybody: the greeter never delivers a response to a noninteractive slot, so the request would hang that slot for the rest of the lock. `gaze doctor` warns when the toggle is on and one of those slots is wired. If you want the confirmation step enforced on a surface that can show a dialog, use simultaneous mode (`pam_gaze.so simultaneous`).
- A **login greeter** is the exception: it never bypasses. GDM always runs GNOME with the Gaze Extension, so confirmation is enforced there or the login is denied.

A "text-based (TTY) environment" means Gaze can open the process's controlling terminal (`/dev/tty`), which is how `sudo` itself finds the terminal to prompt on. Redirected standard input does not change that, so `echo 1 | sudo tee /tmp/1` still confirms from the keyboard. When there is no controlling terminal at all (a management console such as Cockpit that drives PAM over a framed stdio protocol, or a service started without one), nobody can press a key, so Gaze neither prints a terminal banner nor waits for one; the face match is refused and the stack falls through to the password.

Callers that set the PAM `PAM_SILENT` flag, `sudo` among them, receive no messages through their own conversation. Gaze still writes the camera prompt and the verdict to the controlling terminal when there is one, so a terminal user keeps the "Please look at the camera" / "Face Verified." feedback; graphical callers with no terminal stay silent.

With simultaneous mode (`pam_gaze.so simultaneous`):
- The password prompt still comes up immediately so you are never blocked.
- If face verification succeeds before you finish entering your password:
  - In a text-based (TTY) environment, it cancels the password prompt and asks for text confirmation ("Press Enter to confirm, Esc to cancel").
  - In a graphical Polkit environment:
    - On **GNOME and Cinnamon** (with the Gaze Extension active), it hides the password field, focuses the "Authenticate" button, and lets you confirm by pressing Enter or clicking the button. If the extension is inactive, it bypasses confirmation entirely to avoid locking you out.
    - On **KDE Plasma & LXQt**, it prompts you to press "OK" to confirm.
    - On **Hyprland**, it prompts you to press "Authenticate" to confirm.
    - On other graphical environments, it prompts you to press "Enter" to confirm.

`resume_grace_ms` delays face verification on system resume by the specified number of milliseconds (e.g. `3000` ms) to allow slower displays/GPUs to initialize and repaint, preventing verification from occurring behind a blank screen. Set to `0` to disable the delay.

`start_delay_ms` delays face verification by the specified number of milliseconds, not only after suspend. Set to `0` to disable the delay.

A start delay can help if your lock screen unlocks as soon as you lock it. Lockers
start authentication at different times: hyprlock starts its PAM stack when it
launches, and KDE starts scanning as soon as its lock screen appears. If you are
still in front of the camera, Gaze may recognize you and unlock immediately. A
delay of `3000`-`5000` ms gives you time to step away. GNOME does not need this
delay because face authentication begins only after you dismiss the lock shield.

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

After updating the configuration, restart the daemon to apply the changes:

```bash
sudo systemctl restart gazed
```

## Storage paths

Gaze manages these storage locations for you, so you do not need to configure them:

- User embeddings: `/var/lib/gaze/users`
- Duress lockouts: `/var/lib/gaze/duress`
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

## Unlock GNOME Keyring after a GDM or greetd face login

`storage.unlock_gnome_keyring` defaults to `false`. It requires TPM template
encryption and liveness; if either is missing the option is ignored and the
daemon logs why. Run `gaze keyring` for each user after enabling it. Users who
skip it keep logging in with face and are prompted for the keyring as before.
Read the security notes in
[GNOME Keyring setup](/guide/gnome#optional-tpm-backed-keyring-unlock) first:
the stored password is recoverable by root on this machine.

## Unlock KWallet after a KDE face login

`storage.unlock_kwallet` also defaults to `false` and requires both TPM template
encryption and liveness. Enable it for KDE login wallet unlock, then run
`gaze keyring --kwallet` and `sudo gaze-kde-pam enable-login`. Its credential is
independent of GNOME Keyring. See [KWallet setup](/guide/kde#optional-tpm-backed-kwallet-unlock)
for supported login services, PAM setup, and re-enrollment requirements.

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

This setting applies only during enrollment; authentication does not enforce the
same centering and proximity threshold. Choose the highest value that still feels
comfortable, since smaller face crops provide less detail for the enrolled template.

### Multi-camera and hybrid enrollment

Gaze can enroll profiles for both RGB and IR cameras. What it captures depends on your camera configuration when you enroll:

- **Single camera:** If only the RGB camera is configured (the default), Gaze captures and saves RGB templates only.
- **Dual-camera (hybrid) setup:** If both RGB and IR cameras are configured, Gaze captures from both during enrollment. Each step waits for valid, aligned frames from both sensors.

### Upgrading Existing Profiles

If you add or configure an IR camera after enrolling a face, your existing
profiles will contain RGB captures only.
- To see which captures each profile contains, run `gaze list-faces` or open the GUI settings. The `[RGB]` and `[IR]` badges are green when a profile has captures for that spectrum, amber when a camera is configured but the profile has no captures, and grey when no camera is configured.
- To add IR captures to an existing profile, make sure the IR camera is configured, then run:
  ```bash
  gaze refine-face <profile-name>
  ```
  You can also refine the profile in the GUI. Gaze captures the missing spectrum and adds the new templates to the existing profile.

## Liveness Anti-Spoofing

```toml
[liveness]
enabled = true
threshold = 0.8
max_seconds = 2.0
```

When enabled, Gaze runs a local MiniFASNet-V2 anti-spoofing model on the detected face crop after a recognition match. Authentication succeeds only when the face matches and either one frame reaches `threshold` or the best few frames show sustained near-threshold liveness.

Alongside the model, Gaze watches how far your eyes travel between frames, measured against the distance between them so it does not depend on how close you sit. A run that has accumulated several frame pairs and never seen movement above that floor is treated as a still object and refused even when the model is confident. Moving normally (breathing, blinking, small head shifts) clears it; once any pair shows movement, holding still afterwards does not undo it. On IR cameras this movement check is the whole liveness test, since the anti-spoof model is trained on colour frames.

`max_seconds` caps how long (in seconds of usable face frames) Gaze examines the camera before giving up and falling back to your password. It bounds the whole attempt, not just the liveness stage: an unrecognised face spends the same budget. Gaze calculates the frame budget dynamically using your camera's actual frame rate (e.g. 2.0 seconds corresponds to 60 frames on a 30fps webcam, or 120 frames on a 60fps camera). Frames only count while a usable face is in view, and the RGB and IR phases each get the full budget. Raise it if authentication gives up before you are ready; `gaze auth --verbose` reports when a run ends this way.

## Duress Signal

```toml
[duress]
enabled = false
closed_threshold = 0.9
hold_ms = 600
```

Someone can hold your laptop up to your face, but they cannot make you type a password. With duress detection on, you can refuse a forced face unlock with your eyes: keep one eye (or both) closed while the camera sees you.

How it works:

- After your face matches, Gaze runs a small local open/closed eye classifier ([Intel Open Model Zoo `open-closed-eye-0001`](https://github.com/openvinotoolkit/open_model_zoo/tree/master/models/public/open-closed-eye-0001), about 46 KB, Apache-2.0) on each eye. It downloads into `/var/cache/gaze` the first time duress is enabled.
- A matched frame where either eye is closed never unlocks. That alone means a blink cannot let a coerced unlock through.
- If an eye stays closed for `hold_ms`, Gaze rejects the attempt and locks face authentication for that user. While locked, face unlock reports itself as unavailable and falls straight through to the password prompt without touching the camera.
- The lock is stored in `/var/lib/gaze/duress`, so a reboot or daemon restart does not clear it.
- It clears after a successful login that did not use your face, such as your password, on any service where `pam_gaze.so` is in the auth stack (including the `gdm-password`, KDE, and sudo stacks Gaze installs into). You can also clear it with `gaze duress --clear`, which requires fresh administrator authentication.

Settings:

- `closed_threshold` is the classifier's closed-eye probability (0.5 to 0.99) needed to count an eye as shut. Lower it if a deliberate wink is missed; raise it if squinting or laughing trips it.
- `hold_ms` is how long (0 to 5000 ms) the eye must stay shut. A normal blink takes 100 to 400 ms.

The duress table lives only in `/etc/gaze/config.toml`. The GUI and `gaze config` leave it untouched, and the daemon reads it at the start of every face auth, so no restart is needed.

Limits to be aware of:

- Close your eye before the camera sees you. Any matched frame with both eyes open unlocks as usual.
- Another non-face login method, such as a fingerprint, also clears the lock.
- The classifier works on RGB and IR frames, but it is not perfect. Test it with `gaze auth` before relying on it: a held wink should end in a lockout, and `gaze duress --clear` resets it. Running `gazed` with `RUST_LOG=debug` logs the per-eye scores.

## Recommended tuning workflow

1. Start with `[security] level = "medium"`
2. Enroll one profile: `gaze add-face default`
3. Test 5 to 10 times using `gaze auth --verbose`
4. If photo or screen spoofing is a concern, keep `[liveness] enabled = true`
5. If false accepts are too high, switch to `high`
6. If false rejects are too high, run `gaze refine-face default`
