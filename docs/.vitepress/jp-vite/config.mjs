import { posix } from 'node:path'

import { embedSections, lineRange } from './embed.mjs'
import { joinMultilineInlineCode } from './join-inline-code.mjs'
import { metadataHeader } from './meta-header.mjs'
import { devServer } from './middleware.mjs'
import { redirectServer, writeRedirects } from './redirects.mjs'
import { knownDocuments, linkMentions, relativeHref } from './references.mjs'
import { readSettings } from './settings.mjs'
import { slugify } from './slugify.mjs'
import { wikiSidebar } from './wiki.mjs'

// What the RFD, ticket, and wiki pages need from the VitePress configuration.
//
// A project's `.vitepress/config.mts` passes its own configuration through
// `withJpVite`, which adds this to it:
//
//     import { defineConfig } from 'vitepress'
//     import { withJpVite } from './jp-vite/config.mjs'
//
//     export default defineConfig(withJpVite({ title: 'My Project' }))
//
// Hooks the project sets as well (`markdown.preConfig`, `markdown.config`,
// `transformPageData`, `buildEnd`) run after these, and lists are joined, so
// neither side has to know what the other adds. A slug function, a wiki
// sidebar, or an edit link the project sets itself is kept.

// The project's configuration, with the RFD, ticket, and wiki pages' needs
// added.
export function withJpVite(config = {}) {
    const settings = readSettings()
    const markdown = config.markdown ?? {}
    const vite = config.vite ?? {}
    const server = vite.server ?? {}
    const watch = server.watch ?? {}
    const optimizeDeps = vite.optimizeDeps ?? {}

    return {
        ...config,
        themeConfig: withEditLink(withWikiSidebar(config.themeConfig ?? {}, settings), settings),
        markdown: {
            ...markdown,
            // The anchors `jp wiki check` computes, whatever VitePress's
            // default becomes. See `./slugify.mjs`.
            anchor: { slugify, ...markdown.anchor },
            preConfig: chain((md) => {
                // Collapse newlines inside inline backtick spans before any
                // block parsing runs. See `./join-inline-code.mjs` for why.
                md.core.ruler.after('normalize', 'join_multiline_inline_code', (state) => {
                    state.src = joinMultilineInlineCode(state.src)
                })
            }, markdown.preConfig),
            config: chain((md) => {
                escapeInlineCodeInterpolation(md)
                // Before the links are rewritten, so the links an embed adds
                // are rewritten like any other. See `./embed.mjs`.
                embedSections(md)
                rewriteRepositoryLinks(md, settings)
                linkDocumentMentions(md)
                // Last, so the header keeps the links the rules above made.
                compactMetadata(md)
            }, markdown.config),
        },
        // RFDs and tickets aren't in the sidebar, so VitePress's prev/next links
        // would point at whatever page happens to sit next to them in the
        // config. They're indexed documents, not a reading order. Wiki pages
        // are: theirs follow the sidebar.
        transformPageData: chain((pageData) => {
            if (isDocumentPage(pageData.relativePath, readSettings(), ['rfd', 'ticket'])) {
                pageData.frontmatter.prev ??= false
                pageData.frontmatter.next ??= false
            }
        }, config.transformPageData),
        buildEnd: chain(writeRedirects, config.buildEnd),
        vite: {
            ...vite,
            plugins: [redirectServer, ...devServer, ...(vite.plugins ?? [])],
            // What `jp vite serve` found before starting the site, for the
            // pages to show and the boards to obey. Always read-only and
            // trusted in a build.
            define: {
                ...vite.define,
                __JP_VITE_EDITABLE__: JSON.stringify(
                    Boolean(process.env.JP_VITE_SERVE) && !process.env.JP_VITE_PROBLEM,
                ),
                __JP_VITE_PROBLEM__: JSON.stringify(process.env.JP_VITE_PROBLEM || null),
            },
            server: {
                ...server,
                // The boards persist themselves through the dev server, and own
                // their in-memory state. Their files aren't part of the module
                // graph, so a write must not trigger a reload; a manual refresh
                // re-reads them.
                watch: {
                    ...watch,
                    ignored: [
                        '**/.priority.json',
                        '**/.board.json',
                        ...[watch.ignored ?? []].flat(),
                    ],
                },
            },
            // Both boards import SortableJS on demand, which Vite would
            // otherwise discover mid-session and answer with a full page
            // reload. Pre-bundling it means the first visit to a board is just
            // a board.
            optimizeDeps: {
                ...optimizeDeps,
                include: ['sortablejs', ...(optimizeDeps.include ?? [])],
            },
        },
    }
}

// Run `first`, then `second` when the project set one, passing both the same
// arguments. Waits for `first` when it returns a promise.
function chain(first, second) {
    if (!second) return first

    return function (...args) {
        const result = first.apply(this, args)
        if (result && typeof result.then === 'function') {
            return result.then(() => second.apply(this, args))
        }
        return second.apply(this, args)
    }
}

// Whether a page is a document of one of `kinds`, or one of their index pages.
function isDocumentPage(relativePath, settings, kinds = ['rfd', 'ticket', 'wiki']) {
    return kinds.some((kind) => {
        const route = settings[kind]?.route
        return route && relativePath.startsWith(`${route}/`)
    })
}

// `themeConfig` with the wiki's sidebar for the wiki's pages, in reading order.
// See `./wiki.mjs`.
//
// A sidebar the project gives every page stays on every other page. The
// sidebar is read when the configuration loads, so a page added while the dev
// server runs shows in it once the server restarts.
function withWikiSidebar(themeConfig, settings) {
    const sidebar = wikiSidebar(settings)
    if (!sidebar) return themeConfig

    const key = `/${settings.wiki.route}/`
    const existing = themeConfig.sidebar
    if (existing && !Array.isArray(existing) && existing[key]) return themeConfig

    const others = !existing ? {} : Array.isArray(existing) ? { '/': existing } : existing

    return { ...themeConfig, sidebar: { ...others, [key]: sidebar } }
}

// `themeConfig` with a link from each page to the forge's editor for its file
// on the default branch, when the settings name a repository.
//
// Without a default branch, the link opens the file instead, one click from
// editing it: the forge's editor needs a branch's name. On a dev server `jp
// vite serve` started, wiki pages are edited in place as well.
function withEditLink(themeConfig, settings) {
    if (themeConfig.editLink || !settings.repository) return themeConfig

    const dir = settings.site === '.' ? '' : `${settings.site}/`
    const editLink = settings.branch
        ? { pattern: `${settings.repository}/edit/${settings.branch}/${dir}:path`, text: 'Edit this page on GitHub' }
        : { pattern: `${settings.repository}/blob/HEAD/${dir}:path`, text: 'View or edit this page on GitHub' }

    return { ...themeConfig, editLink }
}

// Render the metadata block an RFD, ticket, or wiki page opens with as a
// compact header. See `./meta-header.mjs`.
function compactMetadata(md) {
    md.core.ruler.push('jp_vite_metadata_header', (state) => {
        const page = state.env?.relativePath
        const settings = readSettings()
        if (!page) return

        const kind = ['wiki', 'ticket', 'rfd']
            .find(kind => settings[kind] && page.startsWith(`${settings[kind].route}/`))
        if (!kind) return

        metadataHeader(state.tokens, state.Token, { kind })
    })
}

// Escape `{{ }}` inside inline code spans so Vue's template compiler doesn't
// try to evaluate them. Fenced code blocks are already safe (VitePress adds
// `v-pre` to `<pre>` tags), but inline code (backticks) is not.
function escapeInlineCodeInterpolation(md) {
    md.renderer.rules.code_inline = (tokens, idx) => {
        const escaped = md.utils.escapeHtml(tokens[idx].content)
            .replace(/\{\{/g, '&#123;&#123;')
            .replace(/\}\}/g, '&#125;&#125;')
        return `<code>${escaped}</code>`
    }
}

// Link the RFDs, tickets, and wiki pages an RFD, ticket, or wiki page names in
// its text, such as `RFD 042`, `T-005zd00`, or `W-0a1b2c3#turn`, to their
// pages. A name of a document that doesn't exist stays text. See
// `./references.mjs`.
//
// Runs at parse time, so VitePress's link render rule treats the links like
// ones written by hand. The documents are read on every page, so one created
// while the dev server runs is linked from the next page rendered.
function linkDocumentMentions(md) {
    md.core.ruler.push('link_document_mentions', (state) => {
        const page = state.env?.relativePath
        const settings = readSettings()
        if (!page || !isDocumentPage(page, settings)) return

        const known = knownDocuments(settings)
        linkMentions(state.tokens, state.Token, label => relativeHref(page, known.get(label)))
    })
}

// Rewrite relative links that climb out of the site to the file in the
// repository. Mapping it to the repository lets it resolve on the website and
// skips the dead-link check (which only covers internal links), while the
// markdown source keeps a plain relative link that stays clickable in a
// checkout and on the forge.
//
// Runs at parse time, before VitePress's link render rule, so a rewritten (now
// external) href is treated as external and never recorded for dead-link
// checking.
function rewriteRepositoryLinks(md, settings) {
    md.core.ruler.push('rewrite_repo_links', (state) => {
        const rel = state.env?.relativePath
        if (!rel) return
        const pageDir = posix.dirname(posix.join(settings.site, rel))
        for (const token of state.tokens) {
            if (token.type !== 'inline' || !token.children) continue
            for (const child of token.children) {
                if (child.type !== 'link_open') continue
                const url = repositoryLinkFor(child.attrGet('href'), pageDir, settings)
                if (url) child.attrSet('href', url)
            }
        }
    })
}

// Where a relative link from `pageDir` points in the repository, when it
// leaves the site: `null` for any link VitePress handles itself.
//
// `pageDir` is relative to the workspace root, as `settings.site` is. A
// fragment naming lines, in any form `jp wiki check` reads, is written the way
// a forge reads it: `#L12`, or `#L12-L24`.
export function repositoryLinkFor(href, pageDir, settings) {
    if (!href || !settings.repository) return null
    // Schemes, protocol-relative, anchors, and site-absolute links: leave as-is.
    if (/^(?:[a-z][a-z0-9+.-]*:|\/\/|[#/])/i.test(href)) return null

    const cut = href.search(/[?#]/)
    const linkPath = cut === -1 ? href : href.slice(0, cut)
    const suffix = cut === -1 ? '' : href.slice(cut)

    const resolved = posix.normalize(posix.join(pageDir, linkPath))
    // Inside the site: a normal internal link, handled by VitePress.
    const site = settings.site
    if (site === '.' || resolved === site || resolved.startsWith(`${site}/`)) return null
    // Above the repository root: can't map it, leave it for the dead-link check.
    if (resolved.startsWith('..')) return null

    // A last segment with an extension is a file (`/blob/`); otherwise treat
    // it as a directory (`/tree/`). `HEAD` is the default branch, whatever its
    // name.
    const last = resolved.slice(resolved.lastIndexOf('/') + 1)
    const kind = last.includes('.') ? 'blob' : 'tree'
    return `${settings.repository}/${kind}/HEAD/${resolved}${forgeLines(suffix)}`
}

// `suffix`, the `?query` or `#fragment` of a link, with a fragment naming lines
// written as a forge reads it.
function forgeLines(suffix) {
    const range = suffix.startsWith('#') ? lineRange(suffix.slice(1)) : null
    if (!range) return suffix

    const [start, end] = range
    return start === end ? `#L${start}` : `#L${start}-L${end}`
}
