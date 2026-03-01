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
---

<div class="home-content">

  <section class="install-section">
    <p class="section-label">Quick install</p>
    <div class="install-cmd">
      <code>curl -fsSL https://gaze.gundulabs.com/install.sh | sh</code>
      <button class="copy-btn" onclick="
        navigator.clipboard.writeText('curl -fsSL https://gaze.gundulabs.com/install.sh | sh');
        this.textContent = '✓ Copied';
        this.classList.add('copied');
        setTimeout(() => { this.textContent = 'Copy'; this.classList.remove('copied'); }, 2000);
      ">Copy</button>
    </div>
    <p class="install-note">Supports Fedora, RHEL, Debian, Ubuntu, and Arch Linux</p>
  </section>

  <div class="video-wrapper">
    <video controls muted playsinline :src="'/demo.mp4'"></video>
  </div>

  <section class="features-section">
    <div class="features-grid">
      <div class="feature-card">
        <div class="feature-icon">🔒</div>
        <h3>Privacy-first</h3>
        <p>All inference runs locally using ONNX models. No data ever leaves your machine.</p>
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
  text-align: center;
  padding: 56px 0 64px;
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

.install-cmd {
  display: inline-flex;
  align-items: center;
  gap: 10px;
  background: var(--vp-c-bg-soft);
  border: 1px solid var(--vp-c-border);
  border-radius: 8px;
  padding: 12px 16px;
  font-family: var(--vp-font-family-mono);
  max-width: 560px;
  width: 100%;
}

.install-cmd code {
  flex: 1;
  font-size: 0.9rem;
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
  font-size: 0.78rem;
  cursor: pointer;
  transition: all 0.15s;
}

.copy-btn:hover {
  background: var(--vp-c-brand-soft);
  color: var(--vp-c-brand-1);
  border-color: var(--vp-c-brand-1);
}

.copy-btn.copied {
  background: var(--vp-c-green-soft);
  color: var(--vp-c-green-1);
  border-color: var(--vp-c-green-1);
  transition: all 0.15s;
}

.install-note {
  margin-top: 10px;
  font-size: 0.8rem;
  color: var(--vp-c-text-3);
  margin-bottom: 0;
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
