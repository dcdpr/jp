import { existsSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

import { readSettings } from './settings.mjs'
import { PAGE } from './wiki.mjs'

// Short links, and routes for pages that moved.
//
// - RFDs: `/rfd/101` lands on `/rfd/101-conversation-labels`. The number is the
//   stable part of an RFD's identity — slugs change when a title is reworded —
//   so the number-only form is the one worth pasting into a commit message or
//   an issue.
// - Wiki pages: `/wiki/W-0a1b2c3` and `/wiki/0a1b2c3` land on
//   `/wiki/0a1b2c3-slug`, for the same reason.
// - Moved pages: each route in `plugins.command.vite.options.redirects` lands
//   where it maps to, a route or a wiki page id, so a page renamed later
//   doesn't break the redirect.
//
// Drafts get no short link: draft numbers are recycled once a draft is promoted
// or dropped, so `/rfd/drafts/D38` would resolve to a different document
// depending on when it is opened. Their full paths carry the title slug, which
// makes a stale link obvious rather than silently wrong.
//
// GitHub Pages has no redirect table, so each redirect is a stub page that
// bounces the browser, carrying the query string and `#fragment` across.
// `cleanUrls` means a request for `/rfd/101` is answered with `rfd/101.html`,
// the same way every other page on the site is served.

const RFD = /^(\d{3})-.+\.md$/

// The files of `dir`, sorted; none for a directory that isn't there.
function filesOf(dir) {
    try {
        return readdirSync(dir).sort()
    } catch {
        return []
    }
}

// Every redirect the site serves, as a map from the route asked for to the
// route answered with. `srcDir` is the VitePress source directory.
//
// A moved page naming a wiki page that isn't there is an error: the redirect
// would lead nowhere.
export function siteRedirects(srcDir, settings = readSettings()) {
    const redirects = new Map()

    const rfds = settings.rfd?.route
    if (rfds) {
        for (const file of filesOf(resolve(srcDir, rfds))) {
            const id = file.match(RFD)?.[1]
            // `000` is the templates' shared id: several files, no one target.
            if (!id || id === '000') continue
            redirects.set(`/${rfds}/${id}`, `/${rfds}/${file.replace(/\.md$/, '')}`)
        }
    }

    const wiki = settings.wiki?.route
    const pages = new Map()
    if (wiki) {
        for (const file of filesOf(resolve(srcDir, wiki))) {
            const id = file.match(PAGE)?.[1]
            if (!id) continue
            const target = `/${wiki}/${file.replace(/\.md$/, '')}`
            pages.set(`W-${id}`, target)
            redirects.set(`/${wiki}/${id}`, target)
            redirects.set(`/${wiki}/W-${id}`, target)
        }
    }

    for (const [from, to] of Object.entries(settings.redirects ?? {})) {
        if (!to.startsWith('W-')) {
            redirects.set(from, to)
            continue
        }
        const target = pages.get(to)
        if (!target) {
            throw new Error(
                `\`plugins.command.vite.options.redirects\` sends \`${from}\` to ${to}, and no ` +
                    `wiki page has that id.`,
            )
        }
        redirects.set(from, target)
    }

    return redirects
}

// Where the stub for a redirect from `route` goes in the build output:
// `rfd/101.html` for `/rfd/101`, `old/index.html` for `/old/`.
function stubPath(outDir, route) {
    const path = route.endsWith('/') ? `${route}index` : route

    return resolve(outDir, `${path.slice(1)}.html`)
}

// Emit the redirect stubs into the build output. Call from `buildEnd`.
//
// A stub never replaces a page the build wrote: a redirect from a route that
// still has a page is a mistake, and the build stops on it.
export function writeRedirects(siteConfig) {
    const base = siteConfig.site.base.replace(/\/$/, '')
    for (const [from, to] of siteRedirects(siteConfig.srcDir)) {
        const dest = stubPath(siteConfig.outDir, from)
        if (existsSync(dest)) {
            throw new Error(
                `The redirect from \`${from}\` would replace the page the site has there; drop ` +
                    `it from \`plugins.command.vite.options.redirects\`.`,
            )
        }
        mkdirSync(dirname(dest), { recursive: true })
        writeFileSync(dest, redirectPage(base + to))
    }
}

// The script carries the query string and fragment across, which the meta
// refresh drops; the meta refresh is the fallback when scripts are off.
function redirectPage(url) {
    return `<!doctype html>
<html lang="en-US">
<head>
<meta charset="utf-8">
<title>Redirecting to ${url}</title>
<link rel="canonical" href="${url}">
<meta name="robots" content="noindex">
<meta http-equiv="refresh" content="0; url=${url}">
<script>location.replace(${JSON.stringify(url)} + location.search + location.hash)</script>
</head>
<body><a href="${url}">Redirecting to ${url}</a></body>
</html>
`
}

// A route without the slashes it may end in.
function trimmed(route) {
    return route.replace(/\/+$/, '') || '/'
}

// Where the dev server sends a request for `path`, given `redirects`, or
// `undefined` when it sends it nowhere.
//
// The route as asked for wins, so `/old/` and `/old` can lead to different
// pages; otherwise a trailing slash doesn't matter.
export function redirectFor(redirects, path) {
    if (redirects.has(path)) return redirects.get(path)

    const wanted = trimmed(path)
    return [...redirects].find(([from]) => trimmed(from) === wanted)?.[1]
}

// Dev-server counterpart of the stub pages, so a redirect behaves the same
// locally as it does on the deployed site. The map is rebuilt per request:
// it's a few directory scans, and a page added since then works without a
// restart. A browser keeps the `#fragment` across a redirect on its own.
//
// A redirect to a wiki page that isn't there fails a build. Here it is named
// once in the server's log, and no request is redirected until it is fixed, so
// the rest of the site keeps working while it is.
export const redirectServer = {
    name: 'jp-vite-redirects',
    configureServer(server) {
        let complaint = null
        server.middlewares.use((req, res, next) => {
            const [path, query] = (req.url || '').split('?')
            let redirects
            try {
                redirects = siteRedirects(server.config.root)
                complaint = null
            } catch (error) {
                if (error.message !== complaint) {
                    complaint = error.message
                    server.config.logger.error(`[jp-vite] ${error.message}`)
                }
                return next()
            }

            const target = redirectFor(redirects, path)
            if (!target) return next()

            res.statusCode = 302
            res.setHeader('Location', query ? `${target}?${query}` : target)
            res.end()
        })
    },
}
