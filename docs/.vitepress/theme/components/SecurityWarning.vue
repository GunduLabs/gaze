<template>
  <div class="container">
      <div class="security-warning" role="alert">
        <div class="security-warning-title">Security warning</div>
        <p>
          Gaze can currently be spoofed with a photo. Do not use it as your only authentication
          factor. Liveness detection and IR camera support are planned.
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
  max-width: 1152px;
  margin: 0 auto;
  box-sizing: border-box;
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
