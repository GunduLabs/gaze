<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Threat model

## What this project does and where untrusted input enters

Gaze is face authentication for Linux. A PAM module (`pam_gaze.so`) asks a root daemon (`gazed`) over the system D-Bus to verify the user's face, and the daemon answers with a verdict that decides whether login, `sudo`, polkit, or a lock screen succeeds. Treat every verdict that reaches PAM as a security decision.

Untrusted input enters at:

- **System D-Bus (`com.gundulabs.Gaze`)**: any local user can call the daemon. Caller identity comes from the bus (UID, PID, PAM service), and privileged methods (enroll for another user, delete faces, change config, clear duress, change the GDM profile) are gated by polkit actions in `packaging/config/com.gundulabs.gaze.policy` plus the checks in `crates/gazed/src/daemon/access.rs`.
- **PAM items**: `PAM_USER`, `PAM_SERVICE`, `PAM_RHOST`, `PAM_AUTHTOK`, and module arguments come from the calling application, which may be a network-facing service.
- **Camera frames**: RGB and IR frames from V4L2, GStreamer, or PipeWire. An attacker in front of the camera controls their content (photos, screens, masks, IR spoofs).
- **On-disk state**: `/etc/gaze/config.toml`, `/var/lib/gaze/users` (encrypted face templates), `/var/lib/gaze/keyring` and `/var/lib/gaze/kwallet` (TPM-sealed passwords), `/var/lib/gaze/duress`, `/var/lib/gaze/tpm`, and `/var/cache/gaze` (ONNX models). These are root-owned; a finding that needs prior root write access is out of scope unless it crosses a boundary such as the TPM seal.
- **Model and runtime downloads**: `gazed` fetches InsightFace, MiniFASNet, and eye-state models over HTTPS and loads ONNX Runtime, all pinned by SHA-256.
- **`/proc` of callers**: the SSH heuristic reads `/proc/<pid>/environ` and `comm` up the caller's process tree.

Attackers we care about, in order: a local unprivileged user, a remote user reaching PAM through a network service, a person physically at the machine with a spoof, and a malicious D-Bus client.

## Components that matter most / least

Most important:

- `crates/pam-gaze` (`auth.rs`, `core/`): the PAM entry points, D-Bus client, remote/SSH refusal (`caller_is_remote`, `service_is_network_facing`), and the mapping from daemon replies to PAM return codes. Any path that returns `PAM_SUCCESS` without a matching face is critical.
- `crates/gazed/src/daemon` (`interface.rs`, `access.rs`, `verify.rs`, `config.rs`): D-Bus methods, caller identification, polkit checks, device claiming, and per-user scoping of enrollments and verification.
- `crates/gazed/src/users/`, `crypto.rs`, `tpm/` and `crates/gaze-security`: template storage, AES-GCM encryption under a TPM-sealed key, and the keyring/KWallet password store that releases a user's password after face auth.
- `crates/gazed/src/recognize.rs`, `liveness.rs`, `duress.rs`: match thresholds, anti-spoofing, and the eyes-closed duress signal.
- `crates/gazed/src/models.rs`: download, checksum, and zip extraction of model files into `/var/cache/gaze`.

In scope but less important:

- `crates/gaze-vision`: camera capture, face detection, and alignment. Memory safety bugs in the FFI to OpenCV, GStreamer, or ONNX Runtime here matter.
- `crates/gaze-core`: config parsing, D-Bus types, IR emitter control over V4L2 and I2C.
- `crates/pam-gaze-grosshack`: deprecated, but still shipped and loaded by PAM.

Least important:

- `crates/gaze-cli` and `crates/gaze-gui`: unprivileged clients of the daemon. Bugs here matter only when they let a client make the daemon do something its caller should not.
- `docs/`, `integrations/` (GNOME Shell, KDE, hyprlock extensions), and `packaging/` scripts. Report these only if they weaken the authentication boundary, for example a packaged PAM file that skips the password fallback.

## How to exercise it

- The workspace is built in `/src/target/release`: `gazed`, `gaze`, `gaze-gui`, `libpam_gaze.so`, `libpam_gaze_grosshack.so`. Binaries, the PAM modules, the D-Bus policy, the polkit policy, and `/etc/gaze/config.toml` are installed at their packaged paths.
- Models are pre-fetched into `/var/cache/gaze`, so `gazed` starts without network access. There is no camera or TPM in the image; use the unit tests and mocks rather than expecting live verification.
- `cargo test --workspace --release` runs the unit tests, including the access-control and PAM decision tests.
- `scripts/test-keyring-pam.sh` and `scripts/test-kwallet-pam.sh` drive the shipped PAM stacks through real Linux-PAM with a private confdir and a mock module. `scripts/keyring-pam-harness.c` and `scripts/kde-pam-harness.c` are useful starting points for PAM-level reproducers.
- To talk to the daemon, start a system bus (`dbus-daemon --system --fork`) and `gazed`; it will fail at camera or TPM setup, but the D-Bus surface and access checks can be reached before that.

## How you rate severity

- **Critical**: face authentication succeeds for the wrong user, for no face, or without the daemon (for example PAM returns success on a D-Bus error or timeout); a local unprivileged user gets root or another user's stored password; remote or SSH authentication is accepted by face.
- **High**: a local unprivileged user enrolls, deletes, or replaces another user's faces, changes daemon config, or clears a duress lock without polkit approval; templates or keyring passwords can be decrypted without the TPM; a memory safety bug in the PAM module or daemon reachable from an unprivileged caller; a liveness or duress bypass that needs only a printed photo or phone screen.
- **Medium**: liveness bypass that needs specialist equipment (3D masks, custom IR sources); denial of service of the daemon by an unprivileged local user that blocks password fallback; information leaks across users (who is enrolled, face names) over D-Bus.
- **Low**: denial of service that still lets the password fallback work; issues that require root or physical disk access first; hardening gaps with no demonstrated impact.

Matching accuracy (false accept rate at a given threshold) is a model property, not a bug, unless a code path skips or weakens the configured threshold.

## Anything to leave alone

- Do not report that face recognition is weaker than a password. That is a documented trade-off and the password fallback stays in every shipped PAM stack.
- Do not report findings that need root, the ability to write `/etc/gaze`, `/var/lib/gaze`, or `/etc/pam.d`, or to replace the installed binaries, unless the result crosses the TPM boundary.
- Do not report vulnerabilities in upstream ONNX Runtime, OpenCV, GStreamer, GTK, or the InsightFace models themselves; report how Gaze uses them instead.
- `pam_gaze_grosshack.so` prints a deprecation notice to `/dev/tty` by design.
- The SSH heuristic reading other processes' `/proc` entries is intentional (see `packaging/config/gazed.service`).
