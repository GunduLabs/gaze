---
layout: home

hero:
  name: "Gaze"
  text: "Facial authentication for Linux"
  tagline: On-device face recognition for login, lock screen, sudo, and more.
  image:
    src: /favicon.svg
    alt: Gaze icon
  actions:
    - theme: brand
      text: Install Gaze
      link: /guide/getting-started
    - theme: alt
      text: GitHub
      link: https://github.com/GunduLabs/gaze
features:
  - title: Enroll and test
    details: Capture a profile with gaze add-face default, then verify with gaze auth.
  - title: PAM integration
    details: Works with PAM auth flows and GNOME lock/GDM integration via the extension.
  - title: DBus API
    details: com.gundulabs.Gaze exposes authentication and enrollment for third-party apps.
  - title: Local-first
    details: Runs on your machine with configurable security levels and automatic model download.
---