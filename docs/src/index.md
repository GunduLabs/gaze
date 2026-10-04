---
layout: home

hero:
  name: "Gaze"
  text: "Facial authentication for Linux"
  tagline: Bring facial authentication to Linux with on-device recognition, PAM integration, and tools for login, lock screens, sudo, and desktop management.
  image:
    src: /favicon.svg
    alt: Gaze icon
  actions:
    - theme: brand
      text: Get started
      link: /guide/getting-started
    - theme: alt
      text: Install
      link: /guide/installation
    - theme: alt
      text: GitHub
      link: https://github.com/GunduLabs/gaze

features:
  - title: Quick setup
    details: Install Gaze, start the daemon, enroll your face, and try authentication from the terminal.
    link: /guide/getting-started
    linkText: Start setup
  - title: Desktop login
    details: Set up face unlock for GNOME, KDE Plasma, or Hyprland's hyprlock, or use Gaze with a PAM-based login manager such as SDDM.
    link: /guide/gnome
    linkText: Configure desktop auth
  - title: PAM integration
    details: Use your face for sudo and other programs that authenticate through PAM.
    link: /guide/pam
    linkText: Read the PAM guide
  - title: CLI and GUI tools
    details: Add or remove face profiles, test recognition, and change settings from the terminal or GTK app.
    link: /guide/cli
    linkText: See the CLI
  - title: Local-first
    details: Face templates stay on your machine. The daemon performs recognition locally and communicates with the CLI, GUI, and PAM module over DBus.
    link: /guide/how-it-works
    linkText: How it works
  - title: Troubleshooting
    details: Fix camera selection, daemon startup, DBus permissions, PAM lockouts, and model issues.
    link: /guide/troubleshooting
    linkText: Debug issues
---

<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
