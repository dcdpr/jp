import { readdirSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'

import { canonicalRfd, referencedLabels } from './loaders/rfd-shared.mjs'
import { readMetadata } from './loaders/metadata.mjs'
import { knownDocuments } from './references.mjs'
import { readSettings, workflowDir, workflowPath } from './settings.mjs'

// The wiki, read for the site: its pages, the order they are read in, and the
// sidebar that follows that order.
//
// `jp wiki` writes the pages; this is the site's reader, and it needs less:
// each page's metadata, what it links to, and the order file, `.order.json`.
// The reading order is the one `jp wiki list` prints: each top page, a page
// without a parent, then down its children, siblings in the order file's order
// and then by id. Archived pages are kept, so links to them resolve, and left
// out of the reading order.

// A page's file: its id, seven characters, then its slug.
export const PAGE = /^([0-9a-z]{7})-.+\.md$/

// The file that orders siblings, inside the wiki directory.
export const ORDER_FILE = '.order.json'

const STATUSES = ['Reference', 'Snapshot', 'Archived']

// The characters an id is written in, as `jp` writes them.
const ALPHABET = /^[0-9abcdefghjkmnpqrstvwxyz]{7}$/

// The page `text` names, as `W-0a1b2c3`, or `null` when it names none.
//
// Read the way `jp wiki` reads an id: `W-0a1b2c3`, `W0a1b2c3`, or the bare
// `0a1b2c3`, in any case, with `i` and `l` read as `1` and `o` as `0`.
export function pageId(text) {
    const trimmed = (text ?? '').trim()
    let body = trimmed
    if (/^w/i.test(trimmed)) {
        const rest = trimmed.slice(1)
        // A bare id may itself open with `w`: only a dash, or seven characters
        // left over, says the `w` was the prefix.
        if (rest.startsWith('-') || [...rest].length === 7) body = rest.replace(/^-/, '')
    }

    const folded = [...body.toLowerCase()]
        .map(char => (char === 'i' || char === 'l' ? '1' : char === 'o' ? '0' : char))
        .join('')

    return ALPHABET.test(folded) ? `W-${folded}` : null
}

// The status as the format spells it, matched without case, or as written when
// the format doesn't define it.
function canonicalStatus(status) {
    if (!status) return null

    return STATUSES.find(known => known.toLowerCase() === status.toLowerCase()) ?? status
}

// Parse one page file, named `filename`.
export function parsePage(content, filename) {
    const num = filename.match(PAGE)?.[1] ?? '0000000'
    const meta = readMetadata(content)

    return {
        id: `W-${num}`,
        num,
        title: meta.title ?? filename.replace(/\.md$/, ''),
        status: canonicalStatus(meta.get('Status')),
        parent: pageId(meta.get('Parent')),
        supersededBy: meta.get('Superseded by'),
        summary: meta.get('Summary'),
        labels: meta.all('Label'),
        slug: filename.replace(/\.md$/, ''),
    }
}

// No page ranked anywhere.
const UNRANKED = Object.freeze({ top: [], children: {} })

// The order file in `dir`, as `{ top, children }`: the ranked top pages, and
// each list of ids keyed by the page they sit under.
//
// A wiki without one has ranked nothing yet. One that is there and isn't an
// order is an error, as it is to `jp wiki`.
export function readOrder(dir) {
    const path = resolve(dir, ORDER_FILE)
    let source
    try {
        source = readFileSync(path, 'utf-8')
    } catch {
        return UNRANKED
    }
    if (source.trim() === '') return UNRANKED

    let raw
    try {
        raw = JSON.parse(source)
    } catch (error) {
        throw new Error(`${path} is not an order: ${error.message}.`)
    }

    const lists = (value) => {
        const out = {}
        for (const [key, ids] of Object.entries(value ?? {})) {
            if (Array.isArray(ids)) out[key] = ids.map(String)
        }
        return out
    }
    const children = {}
    for (const [parent, ids] of Object.entries(lists(raw?.children))) {
        const id = pageId(parent)
        if (id) children[id] = ids
    }

    const top = Array.isArray(raw?.top) ? raw.top.map(String) : []

    return { top, children }
}

// `siblings`, page ids, in the order `ranked` gives them: the ranked ones first,
// in its order, then the rest by id. A page ranked twice takes its last place.
export function arrange(ranked, siblings) {
    const ranks = new Map()
    ranked.forEach((entry, rank) => {
        const id = pageId(entry)
        if (id) ranks.set(id, rank)
    })
    const rank = id => (ranks.has(id) ? ranks.get(id) : Infinity)

    return [...siblings].sort((a, b) => {
        if (rank(a) !== rank(b)) return rank(a) < rank(b) ? -1 : 1
        return a < b ? -1 : a > b ? 1 : 0
    })
}

// The active pages in reading order, each as `{ depth, page }`, the depth
// counted from the top.
//
// A page sits under its parent when that is an active page other than itself,
// and at the top otherwise. A page whose parents loop back to it is laid out
// once the rest are placed, so every page appears once.
export function layout(pages, order) {
    const active = pages.filter(page => page.status !== 'Archived')
    const ids = new Set(active.map(page => page.id))
    const byId = new Map(active.map(page => [page.id, page]))
    const parentOf = page =>
        page.parent && page.parent !== page.id && ids.has(page.parent) ? page.parent : null

    const out = []
    const placed = new Set()
    const descend = (id, depth) => {
        if (placed.has(id)) return
        placed.add(id)
        const page = byId.get(id)
        if (!page) return

        out.push({ depth, page })
        const children = active.filter(child => parentOf(child) === id).map(child => child.id)
        for (const child of arrange(order.children?.[id] ?? [], children)) {
            descend(child, depth + 1)
        }
    }

    const tops = active.filter(page => parentOf(page) === null).map(page => page.id)
    for (const id of arrange(order.top ?? [], tops)) descend(id, 0)
    for (const id of [...ids].filter(id => !placed.has(id)).sort()) descend(id, 0)

    return out
}

// Ids claimed by more than one file, which leave every link to them ambiguous.
function findDuplicateIds(files) {
    const byId = new Map()
    for (const file of files) {
        const id = file.slice(0, 7)
        byId.set(id, [...(byId.get(id) ?? []), file])
    }

    const clashes = [...byId].filter(([, group]) => group.length > 1)
    if (clashes.length === 0) return null

    const report = clashes.map(([id, group]) => `  W-${id}: ${group.join(', ')}`).join('\n')
    return `Duplicate wiki page ids found:\n${report}\n\n` +
        `Each page id must map to exactly one file. Give the losing branch's page a fresh ` +
        `id before merging.`
}

// Every page, ordered by id, and the order file.
//
// Each page carries `path`, where the site serves it, `file`, its path below
// the source directory, and `content`, its text. A project without a wiki, or
// whose wiki directory doesn't exist yet, has no pages.
export function loadWiki(settings = readSettings()) {
    const dir = workflowDir(settings, 'wiki')
    const route = workflowPath(settings, 'wiki')
    const empty = { pages: [], order: UNRANKED }
    if (!dir) return empty

    let names
    try {
        names = readdirSync(dir)
    } catch {
        return empty
    }

    const files = names.filter(name => PAGE.test(name)).sort()
    const duplicate = findDuplicateIds(files)
    if (duplicate) throw new Error(duplicate)

    const pages = files.map(name => {
        const content = readFileSync(resolve(dir, name), 'utf-8')
        return {
            ...parsePage(content, name),
            path: `${route}/${name.replace(/\.md$/, '')}`,
            file: `${settings.wiki.route}/${name}`,
            content,
        }
    })

    return { pages, order: readOrder(dir) }
}

// The documents a page links to, as the labels `theme/documents.mjs` keys on:
// what its text names or links to, embeds included, and the RFDs it names under
// the relevant key, `relevant-rfd=116` by default.
function linksOf(page, known, relevantKey) {
    const labels = new Set(referencedLabels(page.content, known))
    for (const label of page.labels) {
        const split = label.indexOf('=')
        if (split < 0 || label.slice(0, split).toLowerCase() !== relevantKey.toLowerCase()) {
            continue
        }
        const num = canonicalRfd(label.slice(split + 1))
        if (num) labels.add(num)
    }
    labels.delete(page.id)

    return [...labels]
}

// Every page as the wiki index, the child lists, and the cross-reference
// widgets read it: its metadata, where the site serves it, what it links to,
// and where it sits in the reading order.
//
// `position` counts from 0 down the reading order, `depth` from the top, and
// `area` is the title of the top page the page sits under, its own for a top
// page; all three are `null` for an archived page, which the reading order
// leaves out.
export function wikiEntries(settings = readSettings()) {
    const { pages, order } = loadWiki(settings)
    const known = knownDocuments(settings)
    const relevantKey = settings.wiki?.relevantKey ?? 'relevant-rfd'

    const placed = new Map()
    let area = null
    layout(pages, order).forEach(({ depth, page }, position) => {
        if (depth === 0) area = page.title
        placed.set(page.id, { depth, position, area })
    })

    return pages.map(page => {
        const { content: _content, file: _file, ...entry } = page
        const { depth = null, position = null, area = null } = placed.get(page.id) ?? {}
        return { ...entry, depth, position, area, links: linksOf(page, known, relevantKey) }
    })
}

// The wiki's sidebar, in reading order, or `null` for a project without a
// wiki. See `sidebarOf`.
export function wikiSidebar(settings = readSettings()) {
    if (!settings.wiki) return null

    const { pages, order } = loadWiki(settings)

    return sidebarOf(pages, order, settings.wiki)
}

// The sidebar of the wiki `wiki`, the settings' section, holding `pages`
// ordered by `order`: the front page and the list of every page, then each top
// page above the pages under it.
export function sidebarOf(pages, order, wiki) {
    const tops = []
    const stack = []
    for (const { depth, page } of layout(pages, order)) {
        const item = { text: page.title, link: page.path }
        if (depth === 0) {
            tops.push(item)
        } else {
            const parent = stack[depth - 1]
            parent.items = [...(parent.items ?? []), item]
        }
        stack[depth] = item
        stack.length = depth + 1
    }

    return [
        { text: 'Overview', link: `/${wiki.route}/` },
        { text: 'All pages', link: `/${wiki.route}/all` },
        ...tops,
    ]
}
