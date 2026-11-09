import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config
export default defineConfig({
  lang: 'en-US',
  title: 'NestBruno',
  description:
    'Keep a Bruno API collection in sync with your NestJS routes — statically, without running your application.',
  ignoreDeadLinks: false,
  themeConfig: {
    // https://vitepress.dev/reference/default-theme-config
    nav: [
      { text: 'Guide', link: '/guide/' },
      { text: 'Commands', link: '/guide/commands' },
      { text: 'Configuration', link: '/guide/configuration' },
      { text: 'Phases', link: '/phases/' }
    ],
    logo: '/logo.svg',

    sidebar: {
      '/guide/': [
        {
          text: 'Guide',
          items: [
            { text: 'Introduction', link: '/guide/' },
            { text: 'Getting Started', link: '/guide/getting-started' },
            { text: 'Commands', link: '/guide/commands' },
            { text: 'Configuration', link: '/guide/configuration' },
            { text: 'DTO Resolution', link: '/guide/dto-resolution' },
            { text: 'Prefix & Versioning', link: '/guide/routes' },
            { text: 'Synchronization Model', link: '/guide/synchronization' },
            { text: 'Watch Mode', link: '/guide/watch-mode' },
            { text: 'CI (GitHub Actions)', link: '/guide/github-actions' },
            { text: 'Safety & Ownership', link: '/guide/safety' },
            { text: 'Benchmarks', link: '/guide/benchmarks' },
            { text: 'Troubleshooting', link: '/guide/troubleshooting' },
            { text: 'Limitations & FAQ', link: '/guide/limitations' }
          ]
        }
      ],
      '/phases/': [
        {
          text: 'Development Phases',
          items: [
            { text: 'Overview', link: '/phases/' },
            { text: 'Phase 1 — Foundation', link: '/phases/phase-01' },
            { text: 'Phase 2 — Controller Discovery', link: '/phases/phase-02' },
            { text: 'Phase 3 — Route Extraction', link: '/phases/phase-03' },
            { text: 'Phase 4 — Bruno Generation', link: '/phases/phase-04' },
            { text: 'Phase 5 — Synchronization', link: '/phases/phase-05' },
            { text: 'Phase 6 — Watch Mode', link: '/phases/phase-06' },
            { text: 'Phase 7 — Staged Synchronization', link: '/phases/phase-07' },
            { text: 'Phase 11 — Cross-File DTO Resolution', link: '/phases/phase-11' }
          ]
        }
      ]
    },

    socialLinks: [
      { icon: 'github', link: 'https://github.com/Silent-Watcher/nestbruno' }
    ],

    footer: {
      message: 'Released under the MIT License.',
      copyright:
        'Copyright © 2026 <a href="https://github.com/Silent-Watcher/nestbruno">NestBruno Dev</a>'
    },

    search: {
      provider: 'local',
      options: {
        translations: {
          button: { buttonText: 'Search docs', buttonAriaLabel: 'Search docs' }
        },
        detailedView: true
      }
    },

    outline: { level: [2, 3], label: 'On this page' },
    docFooter: { prev: 'Previous', next: 'Next' },
    lastUpdated: { text: 'Updated' },
    returnToTopLabel: 'Back to top',
    sidebarMenuLabel: 'Menu'
  }
})
