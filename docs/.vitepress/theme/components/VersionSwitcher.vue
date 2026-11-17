<!-- SPDX-FileCopyrightText: 2026 Gundu Labs -->
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

<script setup lang="ts">
import UpstreamVersionSwitcher from '@viteplus/versions/components/version-switcher.component.vue'

const props = defineProps<{
  screenMenu?: boolean
}>()
</script>

<template>
  <ClientOnly>
    <UpstreamVersionSwitcher
      :screen-menu="screenMenu"
    />
  </ClientOnly>
</template>
