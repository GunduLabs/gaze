# Gaze vs. Other Linux Face Auth

Gaze is one of several projects bringing Windows Hello-style facial
authentication to Linux. They all plug into PAM, they are all open source (MIT),
and they all solve the same basic problem: log in with your face instead of a
password. They differ in how far they push security, how they are architected,
and how much of the desktop they cover.

This page compares Gaze with the three most common alternatives,
[Howdy](https://github.com/boltgolt/howdy),
[Visage](https://github.com/sovren-software/visage), and
[Biopass](https://github.com/TickLabVN/biopass), as fairly as we can. It is
written by the maintainers of Gaze, so treat it as informed and neutral to the best of our abilities; however, make sure to verify anything that matters to you against each project's own docs.

## At a glance

| | **Gaze** | **Howdy** | **Visage** | **Biopass** |
|---|---|---|---|---|
| Language | Rust | Python + C | Rust | C++ + Tauri |
| Face detection | SCRFD | dlib (HOG/CNN) | SCRFD | YOLOv8-face |
| Recognition | ArcFace / ResNet50 | dlib ResNet (128-d) | ArcFace (`w600k_r50`) | EdgeFace |
| Inference runtime | ONNX Runtime | dlib | ONNX Runtime | ONNX Runtime |
| IR camera support | Yes, hybrid RGB+IR combining | Yes | Yes, built-in UVC emitter | Yes |
| Fingerprint | No | No | No | Yes |
| Architecture | Root daemon + system DBus | Subprocess per auth (no daemon) | Daemon + DBus | Daemon + DBus |
| Model reload per auth | No (warm daemon) | Yes (cold each time) | No (warm daemon) | No (warm daemon) |
| Templates at rest | Optional TPM-sealed encryption | Plaintext encodings | SQLite store | Local store |
| Model integrity | SHA-256 verified on download | Bundled | SHA-256 pinned | Bundled |
| Interfaces | CLI + GTK GUI + GNOME ext + Hyprland | CLI only | CLI only | GUI (Tauri) + CLI |
| Desktop integration | GNOME lock screen, Hyprland/hyprlock | PAM only | PAM only | PAM + polkit |
| Configuration | `config.toml` + CLI + GUI | Manual ini file | Config file / Nix | GUI |
| Guided multi-angle enrollment | Yes | Basic | Basic | Basic |
| Built-in health check | Yes (one `doctor` command) | Partial | Partial | GUI status |
| Camera sources | GStreamer / PipeWire | V4L2 device path | V4L2 device path | V4L2 device path |
| Runs on older (pre-AVX2) CPUs | Yes (clients don't link ML runtime) | Yes | Varies | Varies |
| Packaging | deb / rpm / Arch / script | deb / AUR / COPR | deb / Nix / AUR | deb / rpm / AUR |
| License | MIT | MIT | MIT | MIT |

## Liveness and anti-spoofing

Liveness detection is the biggest security difference between these projects.
It's what prevents someone from unlocking your machine with a photo or video of
you.

| | Approach | Printed photo | Screen replay | Video replay |
|---|---|---|---|---|
| **Gaze** | MiniFASNet-V2 CNN (RGB) + eye-motion check (IR) | Blocked | Blocked | Partial |
| **Howdy** | None (only skips over-dark frames) | Not blocked | Not blocked | Not blocked |
| **Visage** | Landmark-stability, zero-model eye micro-movement | Blocked | Not blocked | Not blocked |
| **Biopass** | MobileNetV3 anti-spoof CNN + IR-camera check | Blocked | Blocked | Partial |

Gaze and Biopass run an actual anti-spoofing model on the face crop, so they
reject both photos and screens. Visage's check is motion-only and, by its own
docs, does not stop video replay. Howdy does no active anti-spoofing and its own
README warns it "is in no way as secure as a password." No project fully defeats
a high-quality video replay or 3D mask, which is why a password fallback stays
recommended everywhere, *Gaze included*.

## A note on the alternatives

All of these projects **are** worth your respect! They are OSS, maintained by
developers solving a real problem in the Linux desktop, and any of them can give you
face authentication that works. Howdy defined the category and is the most widely
packaged; Visage is a clean Rust daemon with strong IR handling; Biopass brings
fingerprint and a polished GUI. This page highlights where Gaze differs, but the
right choice is the one that fits your hardware, desktop, and threat model, and
we'd rather you use the one that fits you best than blindly trusting Gaze.
