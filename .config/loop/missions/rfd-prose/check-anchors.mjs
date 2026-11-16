#!/usr/bin/env node
//
// Report `#fragment` links into RFDs that a change between two revisions broke.
//
//   node check-anchors.mjs <base> <head> [file...]
//
// Checks each RFD changed between <base> and <head> under `docs/rfd/`, or each
// file named. For each one it looks at every link to one of its headings: the
// RFD's own in-page links, and links from other Markdown files in the
// repository. A link is broken by the change when its fragment names no heading
// at <head> and the same link from the same file resolved at <base>. Links that
// were already broken at <base> are listed separately and do not fail the run.
//
// Exits 0 when nothing broke, 1 when something did, 2 on bad usage. Anchors
// come from the site's own `slugify`, so a fragment accepted here is one the
// docs site renders.

import { execFileSync } from 'node:child_process'
import path from 'node:path'
import { pathToFileURL } from 'node:url'

const [base, head, ...only] = process.argv.slice(2)
if (!base || !head) {
    console.error('usage: check-anchors.mjs <base> <head> [file...]')
    process.exit(2)
}

let root
const run = (args) =>
    execFileSync('git', args, {
        cwd: root,
        encoding: 'utf8',
        maxBuffer: 256 * 1024 * 1024,
        stdio: ['ignore', 'pipe', 'pipe'],
    })
root = run(['rev-parse', '--show-toplevel']).trim()

const { slugify } = await import(
    pathToFileURL(path.join(root, 'docs/.vitepress/jp-vite/slugify.mjs')).href
)

const shown = new Map()
function show(rev, file) {
    const key = `${rev}:${file}`
    if (!shown.has(key)) {
        let text = null
        try {
            text = run(['show', key])
        } catch {
            // The file does not exist at this revision.
        }
        shown.set(key, text)
    }
    return shown.get(key)
}

// Markdown files at `rev` that mention `needle`. `git grep` exits 1 when
// nothing matches, which is not an error here.
const grepped = new Map()
function mentioning(rev, needle) {
    const key = `${rev}\0${needle}`
    if (!grepped.has(key)) {
        let files = []
        try {
            files = run(['grep', '-l', '-F', '-e', needle, rev, '--', '*.md'])
                .split('\n')
                .filter(Boolean)
                .map((line) => line.slice(rev.length + 1))
        } catch (error) {
            if (error.status !== 1) throw error
        }
        grepped.set(key, files)
    }
    return grepped.get(key)
}

const decode = (s) => {
    try {
        return decodeURIComponent(s)
    } catch {
        return s
    }
}

// Lines outside fenced code blocks, with their 1-based line numbers.
function* prose(text) {
    const lines = text.split('\n')
    let fence = null
    for (let i = 0; i < lines.length; i++) {
        const line = lines[i]
        const marker = line.match(/^ {0,3}(`{3,}|~{3,})/)
        if (fence !== null) {
            if (
                marker &&
                marker[1][0] === fence[0] &&
                marker[1].length >= fence.length &&
                line.trim() === marker[1]
            ) {
                fence = null
            }
            continue
        }
        if (marker) {
            fence = marker[1]
            continue
        }
        yield { line, number: i + 1 }
    }
}

// The rendered text of a heading, close enough for its slug: link targets and
// HTML tags dropped. Backticks and emphasis markers are punctuation, which
// `slugify` folds away anyway.
const plain = (s) =>
    s
        .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
        .replace(/\[([^\]]*)\]\[[^\]]*\]/g, '$1')
        .replace(/\[([^\]]*)\]/g, '$1')
        .replace(/<[^>]+>/g, '')

// Every anchor in `text`, numbering repeated slugs `-1`, `-2`, ... the way
// markdown-it-anchor does. A heading ending in `{#id}` takes `id` as its anchor
// instead of a slug of its text.
function anchors(text) {
    const seen = new Set()
    for (const { line } of prose(text)) {
        const heading = line.match(/^ {0,3}#{1,6}(?:\s+(.*?))?\s*$/)
        if (!heading) continue
        const title = (heading[1] ?? '').replace(/\s+#+$/, '')
        const custom = title.match(/\s*\{#([^}\s]+)\}$/)
        if (custom) {
            seen.add(custom[1])
            continue
        }
        const slug = slugify(plain(title))
        let unique = slug
        for (let n = 1; seen.has(unique); n++) unique = `${slug}-${n}`
        seen.add(unique)
    }
    return seen
}

// Links that carry a fragment: inline links and reference definitions.
function links(text) {
    const found = []
    for (const { line, number } of prose(text)) {
        for (const m of line.matchAll(/\]\(\s*<?([^)\s>#]*)#([^)\s>]+)>?(?:\s+"[^"]*")?\s*\)/g)) {
            found.push({ target: m[1], fragment: m[2], number })
        }
        const definition = line.match(/^ {0,3}\[[^\]]+\]:\s*<?([^\s>#]*)#([^\s>]+)/)
        if (definition) {
            found.push({ target: definition[1], fragment: definition[2], number })
        }
    }
    return found
}

// The repository path a link target names, or `null` for an external URL.
function resolve(source, target) {
    if (target === '') return source
    if (/^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('/')) return null
    let file = path.posix.normalize(path.posix.join(path.posix.dirname(source), decode(target)))
    file = file.replace(/\.html$/, '')
    return file.endsWith('.md') ? file : `${file}.md`
}

// The anchor in `slugs` that `fragment` most likely meant: the one sharing the
// longest prefix once punctuation is ignored, so `base_configjson` finds
// `base-config-json`. `null` when nothing shares a meaningful prefix.
function closest(fragment, slugs) {
    const squash = (s) => s.toLowerCase().replace(/[^a-z0-9]/g, '')
    const target = squash(fragment)
    let best = null
    let bestLength = 0
    for (const slug of slugs) {
        const candidate = squash(slug)
        let n = 0
        while (n < target.length && n < candidate.length && target[n] === candidate[n]) n++
        if (n > bestLength) {
            best = slug
            bestLength = n
        }
    }
    return bestLength >= Math.min(8, target.length) ? best : null
}

// Links at `rev` into `file` whose fragment names no heading in it, keyed by
// source file and fragment so line shifts between revisions don't matter.
function broken(rev, file) {
    const result = new Map()
    const text = show(rev, file)
    if (text === null) return result
    const slugs = anchors(text)
    const sources = new Set([file, ...mentioning(rev, path.posix.basename(file, '.md'))])
    for (const source of sources) {
        const sourceText = show(rev, source)
        if (sourceText === null) continue
        for (const link of links(sourceText)) {
            if (resolve(source, link.target) !== file) continue
            const fragment = decode(link.fragment)
            if (slugs.has(fragment)) continue
            result.set(`${source}\0${fragment}`, {
                source,
                fragment,
                number: link.number,
                suggestion: closest(fragment, slugs),
            })
        }
    }
    return result
}

const files = only.length
    ? only
    : run(['diff', '--name-only', base, head, '--', 'docs/rfd'])
          .split('\n')
          .filter((f) => f.endsWith('.md'))

const introduced = []
const preexisting = []
for (const file of files) {
    const before = broken(base, file)
    for (const [key, link] of broken(head, file)) {
        ;(before.has(key) ? preexisting : introduced).push({ ...link, file })
    }
}

const describe = ({ source, number, fragment, file, suggestion }) =>
    `${source}:${number}: #${fragment} names no heading in ${file}` +
    (suggestion ? `; the site's anchor is #${suggestion}` : '')

if (preexisting.length) {
    console.log(`Already broken at ${base} (${preexisting.length}):`)
    for (const link of preexisting) console.log(`  ${describe(link)}`)
}
if (introduced.length) {
    console.log(`Broken between ${base} and ${head} (${introduced.length}):`)
    for (const link of introduced) console.log(`  ${describe(link)}`)
    process.exit(1)
}
