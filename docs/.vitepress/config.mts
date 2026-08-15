import { execFile } from 'node:child_process'
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, posix, resolve } from 'node:path'

import { defineConfig } from 'vitepress'
import abnfGrammar from './grammars/abnf.tmLanguage.json'
import { joinMultilineInlineCode } from './join-inline-code.mjs'
import { rfdRedirectServer, writeRfdRedirects } from './rfd-redirects.mjs'

// Rewrite relative links that climb above the docs root to absolute GitHub
// URLs. The repo tree is the parent of the docs root, so any link resolving
// outside `docs/` points at a real repository path. Mapping it to github.com
// lets it resolve on the website and skips dead-link checking (which only
// covers internal links), while the markdown source keeps a plain relative
// link that stays clickable locally and on GitHub.
const GITHUB_BASE = 'https://github.com/dcdpr/jp'
const GITHUB_BRANCH = 'main'

function repoLinkFor(href, pageDir) {
    if (!href) return null
    // Schemes, protocol-relative, anchors, and site-absolute links: leave as-is.
    if (/^(?:[a-z][a-z0-9+.-]*:|\/\/|[#/])/i.test(href)) return null

    const cut = href.search(/[?#]/)
    const linkPath = cut === -1 ? href : href.slice(0, cut)
    const suffix = cut === -1 ? '' : href.slice(cut)

    const resolved = posix.normalize(posix.join(pageDir, linkPath))
    // Inside the docs root: a normal internal link, handled by VitePress.
    if (resolved === 'docs' || resolved.startsWith('docs/')) return null
    // Above the repo root: can't map it, leave it for the dead-link check.
    if (resolved.startsWith('..')) return null

    // A last segment with an extension is a file (`/blob/`); otherwise treat
    // it as a directory (`/tree/`).
    const last = resolved.slice(resolved.lastIndexOf('/') + 1)
    const kind = last.includes('.') ? 'blob' : 'tree'
    return `${GITHUB_BASE}/${kind}/${GITHUB_BRANCH}/${resolved}${suffix}`
}

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

// Dev-only write endpoint for the RFD priority board (`/rfd/priority`).
//
// The board page is read-only in the production build. On the dev server it
// POSTs the reordered list here, and this middleware persists it to
// `docs/rfd/.priority.json` in the working tree. The endpoint exists only on the
// dev server, so the static build never carries a write path.
const rfdPriorityWriter = {
    name: 'rfd-priority-writer',
    configureServer(server) {
        server.middlewares.use('/__rfd-priority', (req, res, next) => {
            // Dotted, and deliberately not `priority.json`: a page lives at
            // `/rfd/priority`, and Vite's module resolver would answer a cold
            // document request for it with the JSON transformed into an ES
            // module.
            const file = resolve(server.config.root, 'rfd/.priority.json')

            // The board fetches this on mount to bypass the data loader's
            // dev-lifetime cache, so a refresh reflects the saved file.
            if (req.method === 'GET') {
                res.setHeader('Content-Type', 'application/json')
                try {
                    res.end(readFileSync(file))
                } catch {
                    res.end('{"planned":[],"backlog":[]}')
                }
                return
            }

            if (req.method !== 'POST') return next()

            let body = ''
            let tooBig = false
            req.on('data', (chunk) => {
                body += chunk
                if (body.length > 256 * 1024) {
                    tooBig = true
                    req.destroy()
                }
            })
            req.on('end', () => {
                if (tooBig) {
                    res.statusCode = 413
                    res.end('payload too large')
                    return
                }

                let parsed
                try {
                    parsed = JSON.parse(body)
                } catch {
                    res.statusCode = 400
                    res.end('invalid JSON')
                    return
                }

                const isStrArray = (v) =>
                    Array.isArray(v) && v.every((x) => typeof x === 'string')
                const isGroup = (g) =>
                    g !== null &&
                    typeof g === 'object' &&
                    (g.milestone === null || typeof g.milestone === 'string') &&
                    isStrArray(g.ids)
                if (
                    !Array.isArray(parsed.planned) ||
                    !parsed.planned.every(isGroup) ||
                    !isStrArray(parsed.backlog)
                ) {
                    res.statusCode = 400
                    res.end('expected { planned: [{ milestone, ids }], backlog }')
                    return
                }

                const out = {
                    planned: parsed.planned.map((g) => ({
                        milestone: g.milestone,
                        ids: g.ids,
                    })),
                    backlog: parsed.backlog,
                }
                try {
                    writeFileSync(file, JSON.stringify(out, null, 2) + '\n')
                } catch (err) {
                    res.statusCode = 500
                    res.end(String(err))
                    return
                }

                res.statusCode = 200
                res.setHeader('Content-Type', 'application/json')
                res.end(JSON.stringify({ ok: true }))
            })
        })
    },
}

// Dev-only read/write endpoint for the ticket board (`/ticket/board`).
//
// Two pieces of state, kept apart (see RFD 100): the order within a column
// lives in `docs/ticket/.board.json`, and a ticket's status lives in the ticket
// file. Dragging a card between columns therefore rewrites one `Status` line;
// dragging within a column touches only the board file.
const ticketBoardWriter = {
    name: 'ticket-board-writer',
    configureServer(server) {
        const dir = resolve(server.config.root, 'ticket')
        // Dotted, and deliberately not `board.json`: a page lives at
        // `/ticket/board`, and Vite's module resolver would answer a cold
        // document request for it with the JSON transformed into an ES module.
        const boardFile = resolve(dir, '.board.json')
        const COLUMNS = [
            { key: 'todo', status: 'Todo' },
            { key: 'in_progress', status: 'In Progress' },
            { key: 'done', status: 'Done' },
        ]

        // Every ticket on disk, by id, with the bits the board needs.
        const readTickets = () => {
            const tickets = new Map()
            for (const name of readdirSync(dir)) {
                if (!/^\d{4}-.+\.md$/.test(name)) continue
                const file = resolve(dir, name)
                const content = readFileSync(file, 'utf-8')
                const field = (key) =>
                    content.match(new RegExp(`^- \\*\\*${key}\\*\\*:\\s*(.+)`, 'm'))?.[1]?.trim()
                        ?? null
                const id = `T${name.slice(0, 4)}`
                tickets.set(id, {
                    id,
                    file,
                    title: content.match(/^# T\d+:\s*(.+)/m)?.[1]?.trim() ?? name,
                    status: field('Status'),
                    kind: field('Kind'),
                    blockedBy: field('Blocked by'),
                    implements: field('Implements'),
                    path: `/ticket/${name.replace(/\.md$/, '')}`,
                })
            }
            return tickets
        }

        const readOrder = () => {
            try {
                return JSON.parse(readFileSync(boardFile, 'utf-8'))
            } catch {
                return {}
            }
        }

        server.middlewares.use('/__ticket-board', (req, res, next) => {
            if (req.method === 'GET') {
                const tickets = readTickets()
                const order = readOrder()
                const columns = COLUMNS.map(column => {
                    const rank = new Map((order[column.key] ?? []).map((id, i) => [id, i]))
                    const rows = [...tickets.values()]
                        .filter(t => t.status === column.status)
                        .sort((a, b) => (rank.get(a.id) ?? Infinity) - (rank.get(b.id) ?? Infinity)
                            || a.id.localeCompare(b.id))
                    return { ...column, total: rows.length, tickets: rows }
                })

                res.setHeader('Content-Type', 'application/json')
                res.end(JSON.stringify({ columns }))
                return
            }

            if (req.method !== 'POST') return next()

            let body = ''
            let tooBig = false
            req.on('data', (chunk) => {
                body += chunk
                if (body.length > 256 * 1024) {
                    tooBig = true
                    req.destroy()
                }
            })
            req.on('end', () => {
                if (tooBig) {
                    res.statusCode = 413
                    res.end('payload too large')
                    return
                }

                let parsed
                try {
                    parsed = JSON.parse(body)
                } catch {
                    res.statusCode = 400
                    res.end('invalid JSON')
                    return
                }

                const isStrArray = (v) =>
                    Array.isArray(v) && v.every((x) => typeof x === 'string')
                if (!COLUMNS.every(column => isStrArray(parsed[column.key]))) {
                    res.statusCode = 400
                    res.end('expected { todo, in_progress, done } arrays of ticket ids')
                    return
                }

                try {
                    const tickets = readTickets()
                    const order = {}
                    for (const column of COLUMNS) {
                        order[column.key] = parsed[column.key]
                        for (const id of parsed[column.key]) {
                            const ticket = tickets.get(id)
                            if (!ticket || ticket.status === column.status) continue
                            // The status line is the ticket's own; the first
                            // match is the header, ahead of any quoted example.
                            const content = readFileSync(ticket.file, 'utf-8')
                            writeFileSync(ticket.file, content.replace(
                                /^- \*\*Status\*\*:.*$/m,
                                `- **Status**: ${column.status}`,
                            ))
                        }
                    }
                    writeFileSync(boardFile, JSON.stringify(order, null, 2) + '\n')
                } catch (err) {
                    res.statusCode = 500
                    res.end(String(err))
                    return
                }

                res.statusCode = 200
                res.setHeader('Content-Type', 'application/json')
                res.end(JSON.stringify({ ok: true }))
            })
        })
    },
}

// Dev-only ticket management: add, edit, delete.
//
// Every write shells out to `jp ticket`, which is the one implementation of the
// ticket format. Reimplementing id allocation, slugs, and the file template here
// would be a second writer to keep in step, and they would drift.
const ticketWriter = {
    name: 'ticket-writer',
    configureServer(server) {
        const run = (args) =>
            new Promise((done) => {
                execFile('jp', ['ticket', ...args], {
                    cwd: resolve(server.config.root, '..'),
                }, (error, stdout, stderr) => {
                    done({
                        ok: !error,
                        output: (stdout || stderr || String(error ?? '')).trim(),
                    })
                })
            })

        const json = (res, code, payload) => {
            res.statusCode = code
            res.setHeader('Content-Type', 'application/json')
            res.end(JSON.stringify(payload))
        }

        server.middlewares.use('/__ticket', async (req, res, next) => {
            // A GET reads one ticket back, which is how the edit form gets the
            // current description: the index loader doesn't carry it.
            if (req.method === 'GET') {
                const id = new URL(req.url, 'http://localhost').searchParams.get('id')
                if (!id) return json(res, 400, { ok: false, output: 'missing id' })

                const { ok, output } = await run(['show', id, '--json'])
                if (!ok) return json(res, 500, { ok, output })

                try {
                    return json(res, 200, { ok: true, ...JSON.parse(output) })
                } catch {
                    return json(res, 500, { ok: false, output: 'unreadable ticket JSON' })
                }
            }

            if (req.method !== 'POST') return next()

            let body = ''
            let tooBig = false
            req.on('data', (chunk) => {
                body += chunk
                if (body.length > 256 * 1024) {
                    tooBig = true
                    req.destroy()
                }
            })
            req.on('end', async () => {
                if (tooBig) {
                    res.statusCode = 413
                    res.end('payload too large')
                    return
                }

                let parsed
                try {
                    parsed = JSON.parse(body)
                } catch {
                    res.statusCode = 400
                    res.end('invalid JSON')
                    return
                }

                // Flags are built here rather than taken from the client, so a
                // request can only ask for what the CLI already offers.
                const text = (value) => (typeof value === 'string' ? value : '')
                const flag = (name, value) => (text(value) ? [name, text(value)] : [])
                let args
                switch (parsed.action) {
                    case 'add':
                        if (!text(parsed.kind) || !text(parsed.title)) {
                            res.statusCode = 400
                            res.end('add needs a kind and a title')
                            return
                        }
                        args = ['add', text(parsed.kind), text(parsed.title),
                            ...flag('--body', parsed.body)]
                        break
                    case 'edit':
                        args = ['edit', text(parsed.id),
                            ...flag('--title', parsed.title),
                            ...flag('--body', parsed.body),
                            ...flag('--kind', parsed.kind),
                            ...flag('--status', parsed.status)]
                        break
                    case 'delete':
                        args = ['delete', text(parsed.id)]
                        break
                    default:
                        res.statusCode = 400
                        res.end('expected action: add, edit, or delete')
                        return
                }

                const { ok, output } = await run(args)
                json(res, ok ? 200 : 500, { ok, output })
            })
        })
    },
}

// https://vitepress.dev/reference/site-config

export default defineConfig({
    markdown: {
        languages: [abnfGrammar],
        preConfig(md) {
            // Collapse newlines inside inline backtick spans before any block
            // parsing runs. See `./join-inline-code.mts` for the full rationale
            // (short version: `@mdit-vue/plugin-component` treats any unknown
            // tag at column 0 as a paragraph terminator, which tears apart
            // inline code that happens to wrap mid-line around a `<name>`-like
            // placeholder).
            md.core.ruler.after('normalize', 'join_multiline_inline_code', (state) => {
                state.src = joinMultilineInlineCode(state.src)
            })
        },
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

            // Runs at parse time, before VitePress's link render rule, so a
            // rewritten (now external) href is treated as external and never
            // recorded for dead-link checking.
            md.core.ruler.push('rewrite_repo_links', (state) => {
                const rel = state.env?.relativePath
                if (!rel) return
                const pageDir = posix.dirname(posix.join('docs', rel))
                for (const token of state.tokens) {
                    if (token.type !== 'inline' || !token.children) continue
                    for (const child of token.children) {
                        if (child.type !== 'link_open') continue
                        const url = repoLinkFor(child.attrGet('href'), pageDir)
                        if (url) child.attrSet('href', url)
                    }
                }
            })
        },
    },
    // RFDs and tickets aren't in the sidebar, so VitePress's prev/next links
    // would point at whatever page happens to sit next to them in the config.
    // They're indexed documents, not a reading order.
    transformPageData(pageData) {
        if (/^(rfd|ticket)\//.test(pageData.relativePath)) {
            pageData.frontmatter.prev ??= false
            pageData.frontmatter.next ??= false
        }
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

        writeRfdRedirects(siteConfig)
    },
    vite: {
        plugins: [
            serveMarkdownAsUtf8,
            rfdRedirectServer,
            rfdPriorityWriter,
            ticketBoardWriter,
            ticketWriter,
        ],
        // The priority board persists itself by writing `rfd/.priority.json` via
        // the dev middleware above. It isn't part of the module graph, so it
        // must not trigger a dev-server reload — the board owns its in-memory
        // state and a manual refresh re-reads the file.
        server: {
            watch: {
                ignored: ['**/rfd/.priority.json', '**/ticket/.board.json'],
            },
        },
        // Both boards import SortableJS on demand, which Vite would otherwise
        // discover mid-session and answer with a full page reload. Pre-bundling
        // it means the first visit to a board is just a board.
        optimizeDeps: {
            include: ['sortablejs'],
        },
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
})
