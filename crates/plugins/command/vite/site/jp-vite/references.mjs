import { readdirSync } from 'node:fs'
import { posix, resolve } from 'node:path'

import { SRC_DIR } from './settings.mjs'

// Documents named in prose, and the links the site makes of them.
//
// A ticket or RFD often names another by its id instead of linking to it:
// `RFD 042`, `RFD D12`, `T-005zd00`, or the ticket id alone, `005zd00`. The
// References box above a document lists every document it names, and the
// document's text links each name where it appears, so both read the same
// names the same way.

// A named document: an RFD as `RFD 042` or `RFD D12`, a ticket as `T-005zd00`,
// `T005zd00`, or `005zd00`. The space in `RFD 042` may be a line break, where a
// formatter wrapped it.
//
// Groups: the RFD id; the ticket prefix, if any; the ticket id.
const MENTION = /\bRFD\s+(\d{3}|D\d{2})\b|\b(T-|T)?([0-9a-z]{7})\b/g

// A code span holding nothing but a name, which is linked whole. Same groups.
const WHOLE = /^(?:RFD\s+(\d{3}|D\d{2})|(T-|T)?([0-9a-z]{7}))$/

// The label a name stands for, as `documents.mjs` keys on it: `042`, `D12`,
// `T-005zd00`.
function labelOf(match) {
    return match[1] ?? `T-${match[3]}`
}

// Whether a name says what it is: `RFD 042` and `T-005zd00` do. `005zd00` and
// `T005zd00` read like any seven-letter word or a commit hash, so they name a
// ticket only when that ticket exists.
function isExplicit(match) {
    return match[1] !== undefined || match[2] === 'T-'
}

// The labels of the documents `text` names, each once, in the order they first
// appear.
//
// A bare ticket id counts only when `known`, a map keyed by label such as
// `knownDocuments` gives, holds it; an explicit name always does.
export function mentionedLabels(text, known = new Map()) {
    const labels = new Set()
    for (const match of text.matchAll(MENTION)) {
        const label = labelOf(match)
        if (isExplicit(match) || known.has(label)) labels.add(label)
    }

    return [...labels]
}

// Every RFD and ticket the site has, as the label a name stands for mapped to
// its file below the source directory: `042` to `rfd/042-slug.md`, `T-005zd00`
// to `ticket/005zd00-slug.md`.
//
// A workflow the project doesn't have, or whose directory doesn't exist yet,
// has no documents.
export function knownDocuments(settings, srcDir = SRC_DIR) {
    const known = new Map()
    const add = (route, pattern, label) => {
        let names
        try {
            names = readdirSync(resolve(srcDir, route))
        } catch {
            return
        }
        for (const name of names) {
            const id = name.match(pattern)?.[1]
            if (id) known.set(label(id), `${route}/${name}`)
        }
    }

    if (settings.rfd) {
        add(settings.rfd.route, /^(\d{3})-.+\.md$/, id => id)
        add(`${settings.rfd.route}/drafts`, /^(D\d{2})-.+\.md$/, id => id)
    }
    if (settings.ticket) {
        add(settings.ticket.route, /^([0-9a-z]{7})-.+\.md$/, id => `T-${id}`)
    }
    // The templates, which no name means.
    known.delete('000')

    return known
}

// The link from the page `page` to the file `target`, both below the source
// directory, as markdown writes one; `null` when there is no document to link
// to, or it is the page itself.
export function relativeHref(page, target) {
    if (!target || target === page) return null

    const href = posix.relative(posix.dirname(page), target)
    return href.startsWith('.') ? href : `./${href}`
}

// Turn each document named in the text of `tokens` into a link to it.
//
// `tokens` are markdown-it's block tokens and `Token` its token constructor.
// `hrefFor` gives the link for a label, or `null` to leave the name as text.
// Text already inside a link stays as it is, and so do headings, which a link
// would turn into a link to somewhere else. A code span holding nothing but a
// name, such as `T-005zd00` or `005zd00`, is linked whole.
export function linkMentions(tokens, Token, hrefFor) {
    tokens.forEach((token, index) => {
        if (token.type !== 'inline' || !token.children) return
        if (tokens[index - 1]?.type === 'heading_open') return

        token.children = linkChildren(token.children, Token, hrefFor)
    })
}

function linkChildren(children, Token, hrefFor) {
    const out = []
    let depth = 0

    for (const child of children) {
        if (child.type === 'link_open') {
            depth++
        } else if (child.type === 'link_close') {
            depth--
        } else if (depth === 0 && child.type === 'text') {
            out.push(...linkText(child.content, Token, hrefFor))
            continue
        } else if (depth === 0 && child.type === 'code_inline') {
            const match = child.content.trim().match(WHOLE)
            const href = match ? hrefFor(labelOf(match)) : null
            if (href) {
                out.push(...link(href, [child], Token))
                continue
            }
        }
        out.push(child)
    }

    return out
}

// `text` as text tokens, with each name `hrefFor` links to made a link.
function linkText(text, Token, hrefFor) {
    const out = []
    let from = 0

    for (const match of text.matchAll(MENTION)) {
        const href = hrefFor(labelOf(match))
        if (!href) continue

        if (match.index > from) out.push(textToken(text.slice(from, match.index), Token))
        out.push(...link(href, [textToken(match[0], Token)], Token))
        from = match.index + match[0].length
    }
    if (from < text.length) out.push(textToken(text.slice(from), Token))

    return out
}

function textToken(content, Token) {
    const token = new Token('text', '', 0)
    token.content = content
    return token
}

function link(href, inner, Token) {
    const open = new Token('link_open', 'a', 1)
    open.attrSet('href', href)

    return [open, ...inner, new Token('link_close', 'a', -1)]
}
