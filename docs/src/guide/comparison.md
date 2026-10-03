<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Gaze vs. Other Linux Face Auth

Gaze is one of several projects bringing Windows Hello-style facial
authentication to Linux. They all plug into PAM, they are all open source,
and they all solve the same basic problem: log in with your face instead of a
password. They differ in how far they push security, how they are architected,
and how much of the desktop they cover.

We put this comparison together to show where Gaze differs from
[Howdy](https://github.com/boltgolt/howdy),
[Visage](https://github.com/sovren-software/visage), and
[Biopass](https://github.com/TickLabVN/biopass). If we've missed something or
got a detail wrong, [let us know](https://github.com/GunduLabs/gaze/issues).

## At a glance

| | **Gaze** | **Howdy** | **Visage** | **Biopass** |
|---|---|---|---|---|
| Language | Rust | Python + C | Rust | C++ + Tauri |
| Face detection | SCRFD | dlib (HOG/CNN) | SCRFD | YOLO-Face |
| Recognition | ArcFace (MobileFaceNet / ResNet50) | dlib ResNet (128-d) | ArcFace (`w600k_r50`) | EdgeFace |
| Inference runtime | ONNX Runtime | dlib | ONNX Runtime | ONNX Runtime |
| IR camera support | Yes, hybrid RGB+IR combining, built-in UVC emitter | Yes | Yes, built-in UVC emitter | Yes |
| Fingerprint | No | No | No | Yes |
| Architecture | Root daemon + system DBus | Subprocess per auth (no daemon) | Daemon + DBus | Daemon + DBus |
| Model reload per auth | No (warm daemon) | Yes (cold each time) | No (warm daemon) | No (warm daemon) |
| Templates at rest | Optional TPM-sealed encryption | Plaintext encodings | SQLite store | Local store |
| Keyring unlock after a face login | Yes, optional TPM-sealed credential replayed to `pam_gnome_keyring` | No | No | No |
| Model integrity | SHA-256 verified on download | Bundled | SHA-256 pinned | Bundled |
| Interfaces | CLI (with TUI) + GTK GUI + GNOME/Cinnamon extensions + KDE System Settings | CLI only | CLI + TUI enrollment | GUI (Tauri) + CLI |
| Desktop integration | GNOME, Cinnamon and KDE Plasma lock screens, Hyprland/hyprlock, LightDM, console/TTY login | PAM only | PAM only | PAM + polkit |
| Configuration | `config.toml` + CLI + GUI | Manual ini file | Config file / Nix | GUI |
| Guided multi-angle enrollment | Yes | Basic | Basic | Basic |
| Built-in health check | Yes (one `doctor` command) | Partial | Partial | GUI status |
| Camera sources | GStreamer / PipeWire | V4L2 device path | V4L2 device path | V4L2 device path |
| Runs on older (pre-AVX2) CPUs | No (`gazed` requires AVX2; clients run anywhere) | Yes | Varies | Varies |
| Packaging | deb / rpm / COPR / openSUSE / Arch / Nix / Flatpak / script | deb / AUR / COPR / openSUSE | deb / rpm / Nix / AUR | deb / rpm / AUR |
| License | GPL-3.0-or-later | MIT | MIT | MIT |

## Liveness and anti-spoofing

Liveness detection is the biggest security difference between these projects.
It's what prevents someone from unlocking your machine with a photo or video of
you.

| | Approach | Printed photo | Screen replay | Video replay |
|---|---|---|---|---|
| **Gaze** | MiniFASNet-V2 CNN (RGB) + eye-motion check (IR) | Blocked | Blocked | Partial |
| **Howdy** | None (only skips over-dark frames) | Not blocked | Not blocked | Not blocked |
| **Visage** | Landmark-stability, zero-model eye micro-movement | Not blocked | Not blocked | Not blocked |
| **Biopass** | Anti-spoof model (unnamed upstream) + IR-camera check | Blocked | Blocked | Partial |

Gaze and Biopass run an actual anti-spoofing model on the face crop, so they
reject both photos and screens. Visage's check is motion-only, and its own
README reports that the passive liveness check did not discriminate in testing:
a hand-held phone screen displaced more than two genuine live attempts. Howdy
does no active anti-spoofing and its own README warns it "is in no way as secure
as a password" and that it should not be your sole authentication method. No
project fully defeats a high-quality video replay or 3D mask, which is why a
password fallback stays recommended everywhere, *Gaze included*.

## A note on the alternatives

Any of the three will give you working face authentication. Howdy came first
and is the most widely packaged. Visage is another Rust daemon with IR support,
and its docs explain the limits of its liveness check.
Biopass supports fingerprints as well as faces and has a GUI.

We maintain Gaze, but we'd rather you use what fits your hardware, desktop,
and threat model. Check the security limitations of whichever project you choose.
