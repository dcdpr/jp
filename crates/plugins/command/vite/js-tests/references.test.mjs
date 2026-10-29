import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

import {
    knownDocuments,
    linkMentions,
    mentionedLabels,
    relativeHref,
} from '../site/jp-vite/references.mjs'

// The parts of markdown-it's `Token` the rule uses.
class Token {
    constructor(type, tag, nesting) {
        this.type = type
        this.tag = tag
        this.nesting = nesting
        this.attrs = null
        this.content = ''
        this.children = null
    }

    attrSet(name, value) {
        this.attrs = [[name, value]]
    }
}

function token(type, content = '') {
    const made = new Token(type, '', 0)
    made.content = content
    return made
}

function inline(...children) {
    const made = token('inline')
    made.children = children
    return made
}

function linkOpen(href) {
    const made = new Token('link_open', 'a', 1)
    made.attrSet('href', href)
    return made
}

// An inline token's children as HTML-ish text, to compare whole.
function shown(children) {
    return children
        .map(child => {
            if (child.type === 'link_open') return `<a href="${child.attrs[0][1]}">`
            if (child.type === 'link_close') return '</a>'
            if (child.type === 'code_inline') return `\`${child.content}\``
            return child.content
        })
        .join('')
}

const HREFS = new Map([
    ['042', '../rfd/042-forty-two.md'],
    ['D12', '../rfd/drafts/D12-twelve.md'],
    ['T-0aaaaaa', './0aaaaaa-a.md'],
])
const hrefFor = label => HREFS.get(label) ?? null

test('every name in prose is found once, in order', () => {
    assert.deepEqual(
        mentionedLabels('RFD 042, RFD\nD12 and T-0aaaaaa; RFD 042 again, T-0AAAAAA, xT-0bbbbbb'),
        ['042', 'D12', 'T-0aaaaaa'],
    )
})

test('a bare ticket id names a ticket only when the ticket exists', () => {
    const known = new Map([['T-0aaaaaa', 'ticket/0aaaaaa-a.md']])
    const text = 'Fixed in 0aaaaaa and T0aaaaaa, not 0bbbbbb, release, or commit 8a0593f.'

    assert.deepEqual(mentionedLabels(text, known), ['T-0aaaaaa'])
    assert.deepEqual(mentionedLabels(text), [])
})

test('names of existing documents become links, others stay text', () => {
    const line = inline(token('text', 'See RFD 042 and T-0aaaaaa, not RFD 999 or RFD D12x.'))

    linkMentions([line], Token, hrefFor)

    assert.equal(
        shown(line.children),
        'See <a href="../rfd/042-forty-two.md">RFD 042</a> and ' +
            '<a href="./0aaaaaa-a.md">T-0aaaaaa</a>, not RFD 999 or RFD D12x.',
    )
})

test('text already in a link, and headings, are left alone', () => {
    const linked = inline(linkOpen('elsewhere.md'), token('text', 'RFD 042'), new Token('link_close', 'a', -1))
    const heading = inline(token('text', 'Follow-up to RFD 042'))

    linkMentions([linked, token('heading_open'), heading, token('heading_close')], Token, hrefFor)

    assert.equal(shown(linked.children), '<a href="elsewhere.md">RFD 042</a>')
    assert.equal(shown(heading.children), 'Follow-up to RFD 042')
})

test('a bare ticket id is linked when the ticket exists', () => {
    const line = inline(token('text', 'See 0aaaaaa, T0aaaaaa, and 0bbbbbb; the release.'))

    linkMentions([line], Token, hrefFor)

    assert.equal(
        shown(line.children),
        'See <a href="./0aaaaaa-a.md">0aaaaaa</a>, <a href="./0aaaaaa-a.md">T0aaaaaa</a>, ' +
            'and 0bbbbbb; the release.',
    )
})

test('a code span holding only a name is linked whole', () => {
    const line = inline(
        token('code_inline', 'T-0aaaaaa'),
        token('text', ' and '),
        token('code_inline', '0aaaaaa'),
        token('text', ' but '),
        token('code_inline', 'see T-0aaaaaa'),
    )

    linkMentions([line], Token, hrefFor)

    assert.equal(
        shown(line.children),
        '<a href="./0aaaaaa-a.md">`T-0aaaaaa`</a> and <a href="./0aaaaaa-a.md">`0aaaaaa`</a> ' +
            'but `see T-0aaaaaa`',
    )
})

test('a link is relative to the page, and none is made to the page itself', () => {
    assert.equal(relativeHref('ticket/0aaaaaa-a.md', 'rfd/042-x.md'), '../rfd/042-x.md')
    assert.equal(relativeHref('ticket/0aaaaaa-a.md', 'ticket/0bbbbbb-b.md'), './0bbbbbb-b.md')
    assert.equal(relativeHref('ticket/0aaaaaa-a.md', 'ticket/0aaaaaa-a.md'), null)
    assert.equal(relativeHref('ticket/0aaaaaa-a.md', undefined), null)
})

test('the documents are the RFDs, drafts, and tickets on disk', () => {
    const site = mkdtempSync(join(tmpdir(), 'jp-vite-references-'))
    mkdirSync(join(site, 'rfd/drafts'), { recursive: true })
    mkdirSync(join(site, 'ticket'))
    for (const file of [
        'rfd/000-design-template.md',
        'rfd/042-forty-two.md',
        'rfd/index.md',
        'rfd/drafts/D12-twelve.md',
        'ticket/0aaaaaa-a.md',
        'ticket/board.md',
    ]) {
        writeFileSync(join(site, file), '# A document\n')
    }

    const known = knownDocuments({ rfd: { route: 'rfd' }, ticket: { route: 'ticket' } }, site)

    assert.deepEqual(Object.fromEntries(known), {
        '042': 'rfd/042-forty-two.md',
        'D12': 'rfd/drafts/D12-twelve.md',
        'T-0aaaaaa': 'ticket/0aaaaaa-a.md',
    })
    assert.equal(knownDocuments({ rfd: null, ticket: null }, site).size, 0)
})
