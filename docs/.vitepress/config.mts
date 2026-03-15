import { defineConfig } from 'vitepress'

export default defineConfig({
  ignoreDeadLinks: true,
  title: "Gaze",
  description: "Facial authentication for Linux",
  head: [['link', { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' }]],
  themeConfig: {
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
          { text: 'PAM Setup', link: '/guide/pam' },
          { text: 'GUI Guide', link: '/guide/gui' },
          { text: 'CLI Guide', link: '/guide/cli' },
          { text: 'Configuration', link: '/guide/configuration' },
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
