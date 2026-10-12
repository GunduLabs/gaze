<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Security Policy

Gaze sits on the authentication path of every system it is installed on, so we treat security reports as our highest priority.

## Supported versions

Only the latest release receives security fixes. Please reproduce against it, or against `main`, before reporting.

## Reporting a vulnerability

Do not open a public issue, discussion, or pull request for a vulnerability.

Report it privately by either:

- opening a [GitHub security advisory](https://github.com/GunduLabs/gaze/security/advisories/new), or
- emailing **maintainers@gundulabs.com**.

Please include the Gaze version, your distribution and desktop, the PAM stack involved (for example `/etc/pam.d/sudo` or `gdm-face`), and steps or a proof of concept that reproduces the issue.

We aim to acknowledge reports within 3 days and to ship a fix or mitigation within 30 days for critical and high severity issues. We will credit you in the advisory unless you ask us not to.

## Scope

In scope:

- Face authentication succeeding for the wrong user, without a face, or over a remote session
- Privilege escalation or cross-user access through the `gazed` D-Bus interface or polkit checks
- Decrypting face templates or stored keyring passwords without the TPM
- Memory safety bugs in `pam_gaze.so`, `pam_gaze_grosshack.so`, or `gazed`
- Liveness or duress detection bypasses

Out of scope:

- Attacks that already require root or write access to `/etc/gaze`, `/var/lib/gaze`, or `/etc/pam.d`
- The inherent false accept rate of face recognition at a configured threshold
- Vulnerabilities in upstream dependencies (ONNX Runtime, OpenCV, GStreamer, GTK) that Gaze does not make worse; please report those upstream
