import { defineConfig } from 'vitepress'
import { createHighlighter } from 'shiki'

const INSTALL_CMD = 'curl -fsSL https://gaze.gundulabs.com/install.sh | sh'

const highlightedInstall = await createHighlighter({
  themes: ['github-light', 'github-dark'],
  langs: ['bash'],
}).then(hl => hl.codeToHtml(INSTALL_CMD, {
  lang: 'bash',
  themes: { light: 'github-light', dark: 'github-dark' },
  defaultColor: false,
}))

export default defineConfig({
  vite: {
    plugins: [{
      name: 'install-highlight',
      resolveId(id) { if (id === 'virtual:install-highlight') return id },
      load(id) {
        if (id === 'virtual:install-highlight')
          return `export const html = ${JSON.stringify(highlightedInstall)}; export const command = ${JSON.stringify(INSTALL_CMD)};`
      },
    }],
  },
  ignoreDeadLinks: true,
  title: "Gaze",
  description: "Facial authentication for Linux",
  head: [['link', { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' }]],
  themeConfig: {
    logo: '/favicon.svg',
    nav: [
      { text: 'Home', link: '/' },
      { text: 'Guide', link: '/guide/getting-started' },
    ],

    sidebar: [
      {
        text: 'Guide',
        items: [
          { text: 'Getting Started', link: '/guide/getting-started' },
          { text: 'Installation', link: '/guide/installation' },
          { text: 'Development', link: '/guide/development' },
          { text: 'Contributing', link: '/guide/contributing' },
          {
            text: 'Authentication',
            items: [
              { text: 'PAM', link: '/guide/pam' },
              { text: 'GNOME Extension', link: '/guide/gnome' },
            ]
          },
          { text: 'GUI Guide', link: '/guide/gui' },
          { text: 'CLI Guide', link: '/guide/cli' },
          { text: 'Configuration', link: '/guide/configuration' },
          { text: 'Uninstallation', link: '/guide/uninstallation' },
          { text: 'Troubleshooting', link: '/guide/troubleshooting' },
          { text: 'How Gaze Works', link: '/guide/how-it-works' },
        ]
      }
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/GunduLabs/gaze' }
    ]
  }
})
