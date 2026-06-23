---
layout: home

hero:
  name: "Gaze"
  text: "Facial authentication for Linux"
  tagline: On-device authentication for login, lock screen, sudo, and PAM.
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
    details: Install packages, start gazed, enroll a face, and test authentication from the CLI.
    link: /guide/getting-started
    linkText: Start setup
  - title: Desktop login
    details: Use Gaze with GNOME login, lock screen, GDM, or Hyprland's hyprlock.
    link: /guide/gnome
    linkText: Configure desktop auth
  - title: PAM integration
    details: Add facial authentication to sudo, login managers, and other PAM-backed flows.
    link: /guide/pam
    linkText: Read the PAM guide
  - title: CLI and GUI tools
    details: Enroll, test, remove profiles, and manage authentication from the terminal or GTK app.
    link: /guide/cli
    linkText: See the CLI
  - title: Local-first
    details: Face templates stay on your machine. The root daemon owns the ML pipeline and DBus API.
    link: /guide/how-it-works
    linkText: How it works
  - title: Troubleshooting
    details: Fix camera selection, daemon startup, DBus permissions, PAM lockouts, and model issues.
    link: /guide/troubleshooting
    linkText: Debug issues
---
