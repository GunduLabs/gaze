<template>
  <div class="security-warning" role="alert">
    <div class="security-warning-title">Security warning</div>
    <p>
      Gaze is currently <strong>not suitable for security-critical authentication</strong>.
      It can be spoofed with a simple photo, including one shown on another screen.
    </p>
    <p>
      Liveness detection, IR camera support, and related anti-spoofing protections are planned for upcoming releases.
    </p>
  </div>
</template>

<style scoped>
.security-warning {
  margin: 16px auto 24px;
  max-width: 1152px;
  border: 1px solid #f59e0b;
  border-radius: 12px;
  padding: 18px;
  background: color-mix(in srgb, #f59e0b 14%, var(--vp-c-bg) 86%);
  color: var(--vp-c-text-1);
}

.security-warning-title {
  display: inline-block;
  margin-bottom: 8px;
  font-size: 0.75rem;
  font-weight: 700;
  letter-spacing: 0.08em;
  text-transform: uppercase;
  color: #b45309;
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
