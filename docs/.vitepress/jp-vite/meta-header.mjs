import { encodeLabels } from './theme/label-groups.mjs'

// The metadata block an RFD, ticket, or wiki page opens with, rendered as a
// compact header.
//
//     # Conversation
//
//     - **Status**: Reference
//     - **Date**: 2026-10-30
//     - **Label**: domain=conversation
//     - **Label**: domain=llm
//     - **Summary**: The words the project uses.
//
// The file keeps the list, which reads well in an editor and on a forge. On the
// site, the labels are rendered by `<DocLabels />`, as everywhere else the site
// shows labels. The rest depends on the kind of page:
//
// - A wiki page's status becomes a letter after its title, `R`, `S`, or `A`,
//   and its summary, authors, and date move to the end of the page, in small
//   print, where they are there to look up without standing between the title
//   and the text. Its parent is left to the trail above the title, which
//   `DocCrumbs.vue` draws. A page with no text of its own lists the pages
//   under it in its place.
// - An RFD or ticket keeps one line of status badge, category, date, and
//   authors, and leaves its summary to the index pages.
// - A ticket's header ends with the tickets filed under it, which
//   `<TicketChildren />` lists, and shows nothing for one without children.
//   Its own parent is shown above the title by `DocCrumbs.vue`, as a wiki
//   page's is.
//
// Every other field, such as `Superseded by` or `Extended by`, gets a short
// line of its own under the title and keeps its links.
//
// No Node imports, so the transform can be tested on its own.

// The fields an RFD or ticket shows on its status line, in this order.
const LINE = ['status', 'category', 'date', 'authors']

// The fields a wiki page shows at its end, in this order.
const WIKI_FOOTER = ['summary', 'authors', 'date']

// What each wiki status means, for the letter's tooltip.
const WIKI_STATUS = {
    reference: 'Reference: describes the system as it is, and changes with it',
    snapshot: 'Snapshot: dated research, not kept up to date with the system',
    archived: 'Archived: retired, and kept so links to it still work',
}

const ESCAPES = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }

function escapeHtml(text) {
    return text.replace(/[&<>"']/g, char => ESCAPES[char])
}

// Undo what a formatter escapes in a value: `jp\_config` is `jp_config`.
function unescape(text) {
    return text.replace(/\\([!-/:-@[-`{-~])/g, '$1')
}

// A value as a class name: `In Progress` is `in-progress`.
function classKey(value) {
    return value.toLowerCase().replace(/\s+/g, '-')
}

// One `- **Key**: value` item of the block, read from its paragraph's inline
// token: the key, the value as written, and the value's inline tokens; `null`
// for any other item.
function field(inline) {
    const raw = inline?.content?.match(/^\*\*([^*]+)\*\*:[ \t]*([\s\S]*)$/)
    const children = inline?.children ?? []
    if (!raw || children[0]?.type !== 'strong_open' || children[2]?.type !== 'strong_close') {
        return null
    }

    // The value's tokens follow the key's, its first text opening with `:`.
    const value = children.slice(3)
    if (value[0]?.type === 'text') {
        value[0].content = value[0].content.replace(/^:\s*/, '')
        if (value[0].content === '') value.shift()
    }

    return { key: raw[1].trim(), text: raw[2].replace(/\s+/g, ' ').trim(), children: value }
}

// The items of the list opening at `start`, as fields, and the index of its
// closing token; `null` when an item isn't a field.
function fields(tokens, start) {
    const items = []
    let index = start + 1
    while (index < tokens.length && tokens[index].type !== 'bullet_list_close') {
        if (tokens[index].type !== 'list_item_open') return null

        const paragraph = tokens[index + 1]?.type === 'paragraph_open'
        const read = paragraph ? field(tokens[index + 2]) : null
        if (!read || tokens[index + 3]?.type !== 'paragraph_close') return null
        if (tokens[index + 4]?.type !== 'list_item_close') return null

        items.push(read)
        index += 5
    }

    return index < tokens.length ? { items, end: index } : null
}

// The index of the `#` title, and of the metadata block's list: the first list
// at the top level after the title, before any other heading. Either is `-1`
// when there is none.
function locate(tokens) {
    const title = tokens.findIndex(token => token.type === 'heading_open' && token.tag === 'h1')
    if (title < 0) return { title, block: -1 }

    for (let index = title + 1; index < tokens.length; index++) {
        const token = tokens[index]
        if (token.type === 'heading_open') break
        if (token.level === 0 && token.type === 'bullet_list_open') return { title, block: index }
    }

    return { title, block: -1 }
}

function html(Token, content) {
    const token = new Token('html_block', '', 0)
    token.content = content
    token.block = true
    return token
}

function htmlInline(Token, content) {
    const token = new Token('html_inline', '', 0)
    token.content = content
    return token
}

// A paragraph of class `name` holding `children`, inline tokens.
function paragraph(Token, name, children) {
    const open = new Token('paragraph_open', 'p', 1)
    open.attrSet('class', name)
    open.block = true

    const inline = new Token('inline', '', 0)
    inline.children = children

    const close = new Token('paragraph_close', 'p', -1)
    close.block = true

    return [open, inline, close]
}

// Put a wiki page's status after its title, as a letter in a circle.
//
// It goes before the link the site adds to the heading's own anchor, and
// carries its letter in an attribute the stylesheet shows, so neither the
// page's title nor its search entry gains the letter.
function markTitle(inline, Token, status) {
    const children = inline?.children
    if (!children || !status) return

    const kind = classKey(status)
    const letter = status.charAt(0).toUpperCase()
    const meaning = WIKI_STATUS[kind] ?? status
    const marker = htmlInline(
        Token,
        `<span class="doc-status doc-status--${escapeHtml(kind)}" data-letter="${escapeHtml(letter)}" ` +
            `role="img" aria-label="${escapeHtml(status)}" title="${escapeHtml(meaning)}"></span>`,
    )

    const anchor = children.findIndex(child => child.type === 'link_open' &&
        (child.attrGet?.('class') ?? '').split(/\s+/).includes('header-anchor'))
    if (anchor < 0) {
        children.push(marker)
        return
    }
    // The anchor link follows a space the site adds; the marker goes before it.
    const before = anchor > 0 && children[anchor - 1].type === 'text' && children[anchor - 1].content === ' '
        ? anchor - 1
        : anchor
    children.splice(before, 0, marker)
}

// The status line of an RFD or ticket: a badge for the status, then the
// category, date, and authors, without their email addresses.
function statusLine(found) {
    const parts = []
    for (const key of LINE) {
        const value = found.get(key)
        if (!value) continue

        if (key === 'status') {
            parts.push(`<span class="doc-badge doc-badge--${escapeHtml(classKey(value))}">${escapeHtml(value)}</span>`)
        } else {
            const shown = key === 'authors' ? value.replace(/\s*<[^>]*>/g, '') : value
            parts.push(`<span class="doc-meta-${key}">${escapeHtml(unescape(shown))}</span>`)
        }
    }

    return parts.length ? `<p class="doc-meta-line">${parts.join('')}</p>\n` : ''
}

// The fields at the end of a wiki page, each on a line of its own, keeping the
// links its value holds; authors lose their email addresses.
function footer(Token, fields) {
    const out = []
    for (const key of WIKI_FOOTER) {
        const item = fields.get(key)
        if (!item) continue

        const value = key === 'authors'
            ? [htmlInline(Token, escapeHtml(unescape(item.text.replace(/\s*<[^>]*>/g, ''))))]
            : item.children
        out.push(...paragraph(Token, 'doc-meta-field', [
            htmlInline(Token, `<span class="doc-meta-key">${escapeHtml(item.key)}</span>`),
            ...value,
        ]))
    }
    if (out.length === 0) return []

    return [html(Token, '<div class="doc-meta-footer">\n'), ...out, html(Token, '</div>\n')]
}

// The labels, as the component every page renders them with. `kind` is one of
// a few fixed words, and the labels are encoded, so neither needs escaping.
function labelLine(labels, kind) {
    if (labels.length === 0) return ''

    return `<DocLabels kind="${kind}" labels="${encodeLabels(labels)}" />\n`
}

// Replace the metadata block in `tokens`, a page's block tokens, with its
// compact header; leave a page without one as it is.
//
// `Token` is markdown-it's token constructor, and `kind` the page's kind,
// `wiki`, `rfd`, or `ticket`.
export function metadataHeader(tokens, Token, { kind } = {}) {
    const { title, block: start } = locate(tokens)
    if (start < 0) return

    const block = fields(tokens, start)
    if (!block || block.items.length === 0) return

    const wiki = kind === 'wiki'
    const found = new Map()
    const ending = new Map()
    const labels = []
    const others = []
    for (const item of block.items) {
        const key = item.key.toLowerCase()
        if (key === 'label') {
            if (item.text) labels.push(unescape(item.text))
        } else if (wiki && WIKI_FOOTER.includes(key)) {
            if (!ending.has(key) && item.children.length > 0) ending.set(key, item)
        } else if ((wiki || kind === 'ticket') && key === 'parent') {
            // Shown as the trail above the title.
        } else if (key === 'summary') {
            // Shown by the index pages, not on the page.
        } else if (wiki ? key === 'status' : LINE.includes(key)) {
            if (!found.has(key)) found.set(key, item.text)
        } else if (item.children.length > 0) {
            others.push(item)
        }
    }

    const header = []
    const line = (wiki ? '' : statusLine(found)) + labelLine(labels, kind)
    if (line) header.push(html(Token, line))
    for (const item of others) {
        header.push(...paragraph(Token, 'doc-meta-field', [
            htmlInline(Token, `<span class="doc-meta-key">${escapeHtml(item.key)}</span>`),
            ...item.children,
        ]))
    }
    if (header.length > 0) {
        header.unshift(html(Token, '<div class="doc-meta">\n'))
        header.push(html(Token, '</div>\n'))
    }
    if (kind === 'ticket') header.push(html(Token, '<TicketChildren />\n'))

    const empty = block.end + 1 === tokens.length

    // The title is before the block, so marking it leaves the block's place.
    tokens.splice(start, block.end - start + 1, ...header)
    if (!wiki) return

    markTitle(tokens[title + 1], Token, found.get('status'))
    if (empty) tokens.push(html(Token, '<WikiChildren />\n'))
    tokens.push(...footer(Token, ending))
}
