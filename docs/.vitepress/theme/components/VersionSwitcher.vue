<script setup lang="ts">
import { computed } from 'vue'
import UpstreamVersionSwitcher from '@viteplus/versions/components/version-switcher.component.vue'

interface VersioningPlugin {
  versions: Set<string>
  currentVersion: string
}

const props = defineProps<{
  versioningPlugin: VersioningPlugin
  screenMenu?: boolean
}>()

const versionCollator = new Intl.Collator('en', {
  numeric: true,
  sensitivity: 'base',
})

const sortedVersioningPlugin = computed(() => ({
  ...props.versioningPlugin,
  versions: new Set(
    [...props.versioningPlugin.versions].sort((left, right) =>
      versionCollator.compare(right, left),
    ),
  ),
}))
</script>

<template>
  <UpstreamVersionSwitcher
    :versioning-plugin="sortedVersioningPlugin"
    :screen-menu="screenMenu"
  />
</template>
