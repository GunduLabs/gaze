---
layout: home

hero:
  name: "Gaze"
  text: "Face login for Linux"
  tagline: Install in minutes. Works with PAM, CLI, and GUI.
  actions:
    - theme: brand
      text: Install Gaze
      link: /guide/getting-started
    - theme: alt
      text: GitHub
      link: https://github.com/GunduLabs/gaze
---

<div class="home-content">

<section class="install-section">
<p class="section-label">Quick install</p>

```bash
curl -fsSL https://gaze.gundulabs.com/install.sh | sudo sh
```

<p class="install-note">Supports Fedora, RHEL, Debian, Ubuntu, Arch, and Manjaro</p>
</section>

<section class="steps-section">
  <div class="steps-grid">
    <div class="step-card">
      <div class="step-number">1</div>
      <h3>Install</h3>
      <p>Run the one-line installer or use distro packages.</p>
    </div>
    <div class="step-card">
      <div class="step-number">2</div>
      <h3>Enroll</h3>
      <p>Capture your first face profile with <code>gaze add-face default</code>.</p>
    </div>
    <div class="step-card">
      <div class="step-number">3</div>
      <h3>Test</h3>
      <p>Verify with <code>gaze auth</code> and then enable lock screen flow.</p>
    </div>
  </div>
</section>

  <div class="video-wrapper">
    <video controls muted playsinline :src="'/demo.mp4'"></video>
  </div>

  <section class="features-section">
    <div class="features-grid">
      <div class="feature-card">
        <div class="feature-icon">🔒</div>
        <h3>Private by default</h3>
        <p>Authentication runs on your machine with no account or external service required.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">🔌</div>
        <h3>PAM integration</h3>
        <p>Drop-in PAM module for GDM, lightdm, and any PAM-aware login manager.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">🚌</div>
        <h3>DBus interface</h3>
        <p>org.gaze.Auth exposes authentication and enrollment to any third-party app.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">🖥️</div>
        <h3>GTK4 GUI</h3>
        <p>Adwaita-styled enrollment and authentication interface built with GTK4.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">⚙️</div>
        <h3>Configurable security</h3>
        <p>Four preset security levels — from fast MobileFaceNet to accurate ResNet50.</p>
      </div>
      <div class="feature-card">
        <div class="feature-icon">⬇️</div>
        <h3>Auto model download</h3>
        <p>InsightFace ONNX models are downloaded automatically on first run.</p>
      </div>
    </div>
  </section>

</div>

<style>
.home-content {
  max-width: 1152px;
  margin: 0 auto;
  padding: 0 24px 96px;
}

/* Install section */
.install-section {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 24px 0 64px;
  border-bottom: 1px solid var(--vp-c-divider);
}

.section-label {
  font-size: 0.8rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.1em;
  color: var(--vp-c-text-3);
  margin-bottom: 14px;
}

.install-section div[class*='language-'] {
  width: 100%;
  max-width: 560px;
  margin: 0 !important;
  border-radius: 8px;
}

.install-note {
  margin-top: 10px;
  font-size: 0.8rem;
  color: var(--vp-c-text-3);
  margin-bottom: 0;
}

.steps-section {
  padding: 40px 0 12px;
}

.steps-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 16px;
}

.step-card {
  border: 1px solid var(--vp-c-border);
  border-radius: 12px;
  background: var(--vp-c-bg-soft);
  padding: 18px;
}

.step-number {
  width: 30px;
  height: 30px;
  border-radius: 999px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  font-weight: 700;
  margin-bottom: 8px;
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
}

.step-card h3 {
  margin: 0 0 6px;
}

.step-card p {
  margin: 0;
  color: var(--vp-c-text-2);
  font-size: 0.93rem;
}

.video-wrapper {
  max-width: 800px;
  margin: 40px auto;
  border-radius: 12px;
  overflow: hidden;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.2);
  background: #000;
}

.video-wrapper video {
  width: 100%;
  display: block;
}

/* Features */
.features-section {
  padding-top: 0;
  border-top: 1px solid var(--vp-c-divider);
  padding-top: 64px;
}

.features-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 16px;
}

@media (max-width: 768px) {
  .steps-grid {
    grid-template-columns: 1fr;
  }

  .features-grid {
    grid-template-columns: repeat(2, 1fr);
  }
}

@media (max-width: 480px) {
  .features-grid {
    grid-template-columns: 1fr;
  }
}

.feature-card {
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-border);
  border-radius: 12px;
  padding: 24px;
  transition: border-color 0.25s;
}

.feature-card:hover {
  border-color: var(--vp-c-brand-1);
}

.feature-icon {
  font-size: 1.8rem;
  margin-bottom: 12px;
}

.feature-card h3 {
  font-size: 1rem;
  font-weight: 600;
  color: var(--vp-c-text-1);
  margin-bottom: 8px;
}

.feature-card p {
  font-size: 0.875rem;
  color: var(--vp-c-text-2);
  line-height: 1.6;
  margin: 0;
}
</style>
