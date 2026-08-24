import { mkdirSync, readdirSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

// Short links for RFDs: `/rfd/101` lands on `/rfd/101-conversation-labels`. The
// number is the stable part of an RFD's identity — slugs change when a title is
// reworded — so the number-only form is the one worth pasting into a commit
// message or an issue.
//
// Drafts get no short link: draft numbers are recycled once a draft is promoted
// or dropped, so `/rfd/drafts/D38` would resolve to a different document
// depending on when it is opened. Their full paths carry the title slug, which
// makes a stale link obvious rather than silently wrong.
//
// GitHub Pages has no redirect table, so each short link is a stub page that
// bounces the browser. `cleanUrls` means a request for `/rfd/101` is answered
// with `rfd/101.html`, the same way every other page on the site is served.

const DIR = 'rfd'
const PATTERN = /^(\d{3})-.+\.md$/

// Map every RFD number to its full site path, keyed by the short URL path.
// `srcDir` is the docs root.
export function rfdRedirects(srcDir) {
    const redirects = new Map()
    for (const file of readdirSync(resolve(srcDir, DIR)).sort()) {
        const id = file.match(PATTERN)?.[1]
        // `000` is the templates' shared id: several files, no one target.
        if (!id || id === '000') continue
        redirects.set(`/${DIR}/${id}`, `/${DIR}/${file.replace(/\.md$/, '')}`)
    }
    return redirects
}

// Emit the redirect stubs into the build output. Call from `buildEnd`.
export function writeRfdRedirects(siteConfig) {
    const base = siteConfig.site.base.replace(/\/$/, '')
    for (const [from, to] of rfdRedirects(siteConfig.srcDir)) {
        const dest = resolve(siteConfig.outDir, `${from.slice(1)}.html`)
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

// Dev-server counterpart of the stub pages, so a short link behaves the same
// locally as it does on the deployed site. The map is rebuilt per request:
// it's one directory scan, and a newly added RFD then works without a restart.
export const rfdRedirectServer = {
    name: 'rfd-redirects',
    configureServer(server) {
        server.middlewares.use((req, res, next) => {
            const [path, query] = (req.url || '').split('?')
            const target = rfdRedirects(server.config.root).get(path.replace(/\/+$/, ''))
            if (!target) return next()

            res.statusCode = 302
            res.setHeader('Location', query ? `${target}?${query}` : target)
            res.end()
        })
    },
}
