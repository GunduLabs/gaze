<template>
  <div class="container">
      <div class="security-warning" role="alert">
        <div class="security-warning-title">Security & Liveness</div>
        <p>
          Gaze includes local liveness anti-spoofing and support for infrared (IR) cameras to secure
          authentication against spoofing attacks. For high-security environments, it is recommended
          to keep standard system authentication active as a fallback.
        </p>
      </div>
  </div>
</template>

<style scoped>
.container {
  position: relative;
  padding: 0 24px 24px;
}

@media (min-width: 640px) {
  .container {
    padding: 0 48px 24px;
  }
}

@media (min-width: 960px) {
  .container {
    padding: 0 64px 24px;
  }
}

.security-warning {
  --security-warning-border: var(--vp-c-brand-3);
  --security-warning-bg: var(--vp-c-bg-soft);
  --security-warning-title: var(--vp-c-brand-1);

  max-width: 1152px;
  margin: 0 auto;
  box-sizing: border-box;
  border: 1px solid var(--security-warning-border);
  border-radius: 12px;
  padding: 18px;
  background: var(--security-warning-bg);
  color: var(--vp-c-text-1);
}

:root.dark .security-warning {
  --security-warning-border: var(--vp-c-brand-3);
  --security-warning-bg: var(--vp-c-bg-soft);
  --security-warning-title: var(--vp-c-brand-1);
}

.security-warning-title {
  display: inline-block;
  margin-bottom: 8px;
  font-size: 0.75rem;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: var(--security-warning-title);
}

.security-warning p {
  margin: 0;
  color: var(--vp-c-text-2);
  line-height: 1.55;
}

.security-warning p + p {
  margin-top: 10px;
}
</style>
