<template>
  <div class="install-oneliner">
    <p class="section-label">Quick install</p>
    <div
      class="install-block"
      @mouseenter="hovered = true"
      @mouseleave="hovered = false"
    >
      <button
        class="copy"
        :class="{ copied }"
        :style="{ opacity: hovered || copied ? 1 : 0 }"
        title="Copy Code"
        @click="copyCode"
      />
      <span class="lang">bash</span>
      <div class="shiki-wrap" v-html="html" />
    </div>
    <p class="install-note">Supports Debian, Ubuntu, Fedora, Arch, and Manjaro</p>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { html, command } from 'virtual:install-highlight'

const copied = ref(false)
const hovered = ref(false)

function copyCode() {
  navigator.clipboard.writeText(command).then(() => {
    copied.value = true
    setTimeout(() => { copied.value = false }, 2000)
  })
}
</script>

<style scoped>
.install-oneliner {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 16px 24px 28px;
  max-width: 1152px;
  margin: 0 auto;
}

.section-label {
  font-size: 0.8rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.1em;
  color: var(--vp-c-text-3);
  margin: 0 0 14px;
}

.install-block {
  position: relative;
  width: 100%;
  max-width: 560px;
  border-radius: 8px;
  overflow: hidden;
}

.shiki-wrap :deep(pre) {
  margin: 0;
  padding: 20px 64px 20px 24px;
  border-radius: 8px;
  overflow-x: hidden;
  font-size: 0.9rem;
  line-height: 1.5;
  background-color: var(--vp-code-block-bg) !important;
}

.shiki-wrap :deep(code) {
  font-family: var(--vp-font-family-mono);
}

.install-block .lang {
  position: absolute;
  top: 6px;
  right: 52px;
  font-size: 0.75rem;
  font-family: var(--vp-font-family-mono);
  color: var(--vp-c-text-3);
  pointer-events: none;
  z-index: 2;
}

.install-block button.copy {
  position: absolute;
  top: 8px;
  right: 8px;
  z-index: 3;
  border: 1px solid var(--vp-code-copy-code-border-color);
  border-radius: 4px;
  width: 40px;
  height: 40px;
  background-color: var(--vp-code-copy-code-bg);
  cursor: pointer;
  background-image: var(--vp-icon-copy);
  background-position: 50%;
  background-size: 20px;
  background-repeat: no-repeat;
  transition: border-color 0.25s, background-color 0.25s, opacity 0.25s;
}

.install-block button.copy:hover,
.install-block button.copy.copied {
  border-color: var(--vp-code-copy-code-hover-border-color);
  background-color: var(--vp-code-copy-code-hover-bg);
}

.install-block button.copy.copied {
  border-radius: 0 4px 4px 0;
  background-image: var(--vp-icon-copied);
}

.install-block button.copy.copied::before {
  content: var(--vp-code-copy-copied-text-content, 'Copied');
  position: absolute;
  top: -1px;
  right: 100%;
  display: flex;
  justify-content: center;
  align-items: center;
  border: 1px solid var(--vp-code-copy-code-hover-border-color);
  border-right: 0;
  border-radius: 4px 0 0 4px;
  padding: 0 10px;
  height: 40px;
  white-space: nowrap;
  font-size: 0.8rem;
  background-color: var(--vp-code-copy-code-hover-bg);
  color: var(--vp-c-text-2);
}

.install-note {
  margin: 10px 0 0;
  font-size: 0.8rem;
  color: var(--vp-c-text-3);
}
</style>

<!-- Global styles for shiki span color switching (mirrors VitePress vp-code.css) -->
<style>
.install-block .shiki span {
  color: var(--shiki-light, inherit);
}
.dark .install-block .shiki span {
  color: var(--shiki-dark, inherit);
}
</style>
