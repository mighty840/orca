import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'Orca',
  description: 'Container + Wasm orchestrator with AI ops',
  base: '/orca/',
  head: [['link', { rel: 'icon', href: '/orca/logo.svg' }]],

  appearance: 'dark',

  themeConfig: {
    logo: '/logo.svg',

    nav: [
      { text: 'Home', link: '/' },
      { text: 'Guide', link: '/guide/getting-started' },
      { text: 'API', link: '/reference/api' },
      { text: 'GitHub', link: 'https://github.com/mighty840/orca' },
    ],

    sidebar: [
      {
        text: 'Getting Started',
        items: [
          { text: 'Installation', link: '/guide/getting-started' },
          { text: 'Configuration', link: '/guide/configuration' },
          { text: 'Services', link: '/guide/services' },
        ],
      },
      {
        text: 'Operations',
        items: [
          { text: 'Deployment', link: '/guide/deployment' },
          { text: 'Multi-Node', link: '/guide/multi-node' },
          { text: 'Monitoring', link: '/guide/monitoring' },
        ],
      },
      {
        text: 'AI Ops',
        items: [
          { text: 'AI Assistant', link: '/guide/ai-ops' },
        ],
      },
      {
        text: 'Reference',
        items: [
          { text: 'CLI Commands', link: '/reference/cli' },
          { text: 'REST API', link: '/reference/api' },
          { text: 'Self-Healing', link: '/reference/self-healing' },
        ],
      },
      {
        text: 'Architecture',
        items: [
          { text: 'Overview', link: '/architecture' },
        ],
      },
    ],

    socialLinks: [
      { icon: 'github', link: 'https://github.com/mighty840/orca' },
    ],

    search: {
      provider: 'local',
    },

    footer: {
      message: 'Released under the AGPL-3.0 License.',
      copyright: 'Copyright 2025-present Orca Contributors',
    },
  },
})
