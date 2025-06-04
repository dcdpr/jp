import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config

export default defineConfig({
    lang: 'en-US',
    title: "Jean-Pierre",
    description: "An LLM-based Programming Assistant",
    cleanUrls: true,
    themeConfig: {
        outline: {
            level: [2, 3]
        },
        externalLinkIcon: true,
        search: {
            provider: 'local'
        },
        // https://vitepress.dev/reference/default-theme-config
        nav: [
            { text: 'Home', link: '/' },
            { text: 'Installation', link: '/installation' },
            { text: 'Change Log', link: '/change-log' },
        ],

        sidebar: [
            {
                text: 'Getting Started', link: '/getting-started', items: [
                    { text: 'Installation', link: '/installation' },
                    { text: 'Usage', link: '/usage' },
                    { text: 'Case Studies', link: '/case-studies' },
                ],
            },
            {
                text: 'Configuration', link: '/configuration', items: [
                    { text: 'Features', link: '/features' },
                    { text: 'Attachments', link: '/attachment' },
                    { text: 'Personas', link: '/persona' },
                    { text: 'Contexts', link: '/context' },
                    { text: 'Embedded Tools', link: '/tools' },
                ]
            }
        ],

        socialLinks: [{ icon: 'github', link: 'https://github.com/dcdpr/jp' }],
    }
})
