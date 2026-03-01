---
layout: home

hero:
  name: "Gaze"
  text: "Facial authentication for Linux"
  tagline: Fast, local face recognition via PAM and DBus — no cloud dependency.
  actions:
    - theme: brand
      text: Get Started
      link: /guide/getting-started
    - theme: alt
      text: GitHub
      link: https://github.com/GunduLabs/gaze

features:
  - title: Privacy-first
    details: All inference runs locally using ONNX models. No data ever leaves your machine.
  - title: PAM integration
    details: Drop-in PAM module for GDM, lightdm, and any PAM-aware login manager.
  - title: DBus interface
    details: org.gaze.Auth exposes authentication and enrollment to any third-party app.
  - title: GTK4 GUI
    details: Adwaita-styled enrollment and authentication interface built with GTK4.
  - title: Configurable security
    details: Four preset security levels — from fast MobileFaceNet to accurate ResNet50.
  - title: Auto model download
    details: InsightFace ONNX models are downloaded automatically on first run.
---
