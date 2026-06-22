import DefaultTheme from 'vitepress/theme'
import { h } from 'vue'
import type { Theme } from 'vitepress'
import VersionSwitcher from '@viteplus/versions/components/version-switcher.component.vue'

import SecurityWarning from './components/SecurityWarning.vue'
import InstallOneliner from './components/InstallOneliner.vue'

const theme: Theme = {
  ...DefaultTheme,
  enhanceApp({ app }) {
    app.component('VersionSwitcher', VersionSwitcher)
  },
  Layout: () => {
    return h(DefaultTheme.Layout, null, {
      'home-features-before': () => [h(InstallOneliner), h(SecurityWarning)],
    })
  },
}

export default theme
