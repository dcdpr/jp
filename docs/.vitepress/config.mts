import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

import { defineConfig } from 'vitepress'
import abnfGrammar from './grammars/abnf.tmLanguage.json'

// https://vitepress.dev/reference/site-config

export default defineConfig({
    markdown: {
        languages: [abnfGrammar],
        config(md) {
            // Escape {{ }} inside inline code spans so Vue's template compiler
            // doesn't try to evaluate them. Fenced code blocks are already
            // safe (VitePress adds v-pre to <pre> tags), but inline code
            // (backticks) is not.
            md.renderer.rules.code_inline = (tokens, idx) => {
                const escaped = md.utils.escapeHtml(tokens[idx].content)
                    .replace(/\{\{/g, '&#123;&#123;')
                    .replace(/\}\}/g, '&#125;&#125;')
                return `<code>${escaped}</code>`
            }
        },
    },
    async buildEnd(siteConfig) {
        // Copy raw .md source files into the output directory so every page
        // is also reachable at its .md URL (e.g. /getting-started.md).
        // This lets LLMs and tools fetch clean markdown without parsing HTML.
        for (const page of siteConfig.pages) {
            const src = resolve(siteConfig.srcDir, page)
            const dest = resolve(siteConfig.outDir, page)
            if (!existsSync(src)) continue
            mkdirSync(dirname(dest), { recursive: true })
            copyFileSync(src, dest)
        }
    },
    lang: 'en-US',
    base: '/', // https://jp.computer
    title: "Jean-Pierre",
    description: "An LLM-based Programming Assistant",
    cleanUrls: true,
    srcExclude: ['README/**'],
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
            { text: 'RFDs', link: '/rfd/' },
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
