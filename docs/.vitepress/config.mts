import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

import { defineConfig } from 'vitepress'
import abnfGrammar from './grammars/abnf.tmLanguage.json'
import { withJpVite } from './jp-vite/config.mjs'

// Serve raw `.md` files with an explicit UTF-8 charset on the dev server. The
// production build bakes a BOM into the copied mirrors (see `buildEnd`), but
// the dev server streams source files straight from disk, where Vite labels
// `.md` as `text/markdown` with no charset and browsers fall back to Latin-1.
// Gated to top-level navigations (Accept: text/html) so it never intercepts
// the `.md` module requests VitePress uses to render pages.
const serveMarkdownAsUtf8 = {
    name: 'serve-markdown-as-utf8',
    configureServer(server) {
        server.middlewares.use((req, res, next) => {
            const url = (req.url || '').split('?')[0]
            if (
                req.method === 'GET' &&
                url.endsWith('.md') &&
                (req.headers.accept || '').includes('text/html')
            ) {
                const file = resolve(server.config.root, '.' + decodeURIComponent(url))
                if (file.startsWith(server.config.root) && existsSync(file)) {
                    res.setHeader('Content-Type', 'text/markdown; charset=utf-8')
                    res.end(readFileSync(file))
                    return
                }
            }
            next()
        })
    },
}

// https://vitepress.dev/reference/site-config
//
// `withJpVite` adds what the RFD and ticket pages need, from the files
// `jp vite sync` keeps in `./jp-vite/`: the dev server's board and ticket
// endpoints, RFD short links, the markdown fixes for inline code, and links to
// repository files outside the site. Those files are synced from
// `crates/plugins/command/vite/site/`; change them there.
export default defineConfig(withJpVite({
    markdown: {
        languages: [abnfGrammar],
    },
    async buildEnd(siteConfig) {
        // Copy raw .md source files into the output directory so every page
        // is also reachable at its .md URL (e.g. /getting-started.md).
        // This lets LLMs and tools fetch clean markdown without parsing HTML.
        //
        // Prepend a UTF-8 BOM: GitHub Pages serves `.md` without a
        // `charset=utf-8` Content-Type, so without an in-band signal browsers
        // viewing the raw file fall back to Latin-1 and mangle multi-byte
        // characters (e.g. an em dash renders as `â€”`). The BOM forces UTF-8
        // decoding and is stripped by virtually all markdown/text parsers.
        const BOM = '\uFEFF'
        for (const page of siteConfig.pages) {
            const src = resolve(siteConfig.srcDir, page)
            const dest = resolve(siteConfig.outDir, page)
            if (!existsSync(src)) continue
            mkdirSync(dirname(dest), { recursive: true })
            const content = readFileSync(src, 'utf-8')
            writeFileSync(dest, content.startsWith(BOM) ? content : BOM + content)
        }
    },
    vite: {
        plugins: [serveMarkdownAsUtf8],
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
            { text: 'Tickets', link: '/ticket/' },
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
                    { text: 'Conversation Search', link: '/features/conversation-search' },
                    { text: 'Workspace Tools', link: '/features/tools' },
                    { text: 'Model Context Protocol', link: '/features/mcp' },
                    { text: 'Structured Output', link: '/features/structured-output' },
                ]
            }
        ],

        socialLinks: [{ icon: 'github', link: 'https://github.com/dcdpr/jp' }],
    }
}))
