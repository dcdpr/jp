import { defineConfig } from 'vitepress'

// https://vitepress.dev/reference/site-config

export default defineConfig({
    lang: 'en-US',
    base: '/', // https://jp.computer
    title: "Jean-Pierre",
    description: "An LLM-based Programming Assistant",
    cleanUrls: true,
    srcExclude: ['rfd/**/*'],
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
                    { text: 'Configuration', link: '/configuration' },
                    { text: 'Usage', link: '/usage' },
                    { text: 'Case Studies', link: '/case-studies' },
                ],
            },
            {
                text: 'Features', link: '/features', items: [
                    { text: 'Personas', link: '/features/personas' },
                    { text: 'Named Contexts', link: '/features/contexts' },
                    { text: 'Attachments', link: '/features/attachments' },
                    { text: 'Workspace Tools', link: '/features/tools' },
                    { text: 'Model Context Protocol', link: '/features/mcp' },
                    { text: 'Structured Output', link: '/features/structured-output' },
                ]
            }
        ],

        socialLinks: [{ icon: 'github', link: 'https://github.com/dcdpr/jp' }],
    }
})
