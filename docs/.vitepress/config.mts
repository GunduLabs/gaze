import { defineConfig } from 'vitepress'

export default defineConfig({
  ignoreDeadLinks: true,
  vite: {
    assetsInclude: ['**/*.mp4'],
  },
  title: "Gaze",
  description: "Facial authentication daemon for Linux",
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
          { text: 'Configuration', link: '/guide/configuration' },
          { text: 'CLI Reference', link: '/guide/cli' },
          { text: 'GUI', link: '/guide/gui' },
          { text: 'How It Works', link: '/guide/how-it-works' },
        ]
      }
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/GunduLabs/gaze' }
    ]
  }
})
