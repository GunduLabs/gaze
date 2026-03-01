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
  - icon: 🔒
    title: Privacy-first
    details: All inference runs locally using ONNX models. No data ever leaves your machine.
  - icon: 🔌
    title: PAM integration
    details: Drop-in PAM module for GDM, lightdm, and any PAM-aware login manager.
  - icon: 🚌
    title: DBus interface
    details: org.gaze.Auth exposes authentication and enrollment to any third-party app.
  - icon: 🖥️
    title: GTK4 GUI
    details: Adwaita-styled enrollment and authentication interface built with GTK4.
  - icon: ⚙️
    title: Configurable security
    details: Four preset security levels — from fast MobileFaceNet to accurate ResNet50.
  - icon: ⬇️
    title: Auto model download
    details: InsightFace ONNX models are downloaded automatically on first run.
---

<div class="install-section">
  <div class="install-inner">
    <p class="install-label">Quick install</p>
    <div class="install-cmd">
      <code>curl -fsSL https://gaze.gundulabs.com/install.sh | sh</code>
      <button class="copy-btn" onclick="navigator.clipboard.writeText('curl -fsSL https://gaze.gundulabs.com/install.sh | sh')">Copy</button>
    </div>
    <p class="install-note">Supports Fedora, RHEL, Debian, Ubuntu, and Arch Linux</p>
  </div>
</div>

<div class="video-section">
  <div class="video-inner">
    <h2>See it in action</h2>
    <p>Face enrollment and authentication in under 10 seconds.</p>
    <div class="video-wrapper">
      <!-- Replace with your actual video URL -->
      <iframe
        src="https://www.youtube.com/embed/dQw4w9WgXcQ"
        title="Gaze demo"
        frameborder="0"
        allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture"
        allowfullscreen
      ></iframe>
    </div>
  </div>
</div>

<style>
.install-section {
  padding: 48px 24px 0;
}

.install-inner {
  max-width: 640px;
  margin: 0 auto;
  text-align: center;
}

.install-label {
  font-size: 0.85rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--vp-c-text-3);
  margin-bottom: 12px;
}

.install-cmd {
  display: flex;
  align-items: center;
  gap: 8px;
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-border);
  border-radius: 8px;
  padding: 12px 16px;
  font-family: var(--vp-font-family-mono);
}

.install-cmd code {
  flex: 1;
  font-size: 0.95rem;
  color: var(--vp-c-text-1);
  background: none;
  padding: 0;
  text-align: left;
}

.copy-btn {
  flex-shrink: 0;
  padding: 4px 12px;
  border-radius: 6px;
  border: 1px solid var(--vp-c-border);
  background: var(--vp-c-bg);
  color: var(--vp-c-text-2);
  font-size: 0.8rem;
  cursor: pointer;
  transition: all 0.15s;
}

.copy-btn:hover {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  border-color: var(--vp-c-brand-1);
}

.install-note {
  margin-top: 10px;
  font-size: 0.82rem;
  color: var(--vp-c-text-3);
}

.video-section {
  padding: 64px 24px;
  margin-top: 48px;
}

.video-inner {
  max-width: 800px;
  margin: 0 auto;
  text-align: center;
}

.video-inner h2 {
  font-size: 2rem;
  font-weight: 700;
  margin-bottom: 12px;
  color: var(--vp-c-text-1);
}

.video-inner p {
  color: var(--vp-c-text-2);
  margin-bottom: 32px;
  font-size: 1.1rem;
}

.video-wrapper {
  position: relative;
  padding-bottom: 56.25%;
  height: 0;
  border-radius: 12px;
  overflow: hidden;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.2);
}

.video-wrapper iframe {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
}
</style>
