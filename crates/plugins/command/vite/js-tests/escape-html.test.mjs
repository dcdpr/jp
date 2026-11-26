import assert from 'node:assert/strict'
import { test } from 'node:test'

import { escapeHtml } from '../site/jp-vite/escape-html.mjs'

// The parts of markdown-it's `Token` the rule uses.
class Token {
    constructor(type, tag, nesting) {
        this.type = type
        this.tag = tag
        this.nesting = nesting
        this.level = 0
        this.map = null
        this.content = ''
        this.children = null
    }
}

function token(type, fields = {}) {
    return Object.assign(new Token(type, fields.tag ?? '', fields.nesting ?? 0), fields)
}

// A stand-in for markdown-it's inline parser: a run of text, and each `<...>`
// as an `html_inline` token.
function parseInline(content) {
    return content
        .split(/(<[^>]*>)/)
        .filter(Boolean)
        .map(part => token(part.startsWith('<') ? 'html_inline' : 'text', { content: part }))
}

function paragraph(content, map) {
    return [
        token('paragraph_open', { tag: 'p', nesting: 1, map }),
        token('inline', { content, children: parseInline(content), map }),
        token('paragraph_close', { tag: 'p', nesting: -1 }),
    ]
}

// Each token as `type: content`, children indented under their inline token.
function shape(tokens) {
    return tokens.flatMap(t => [
        t.type === 'inline' ? `inline: ${t.content}` : t.type,
        ...(t.children ?? []).map(c => `  ${c.type}: ${c.content}`),
    ])
}

test('a placeholder inside a paragraph is text', () => {
    const tokens = paragraph('Pass `--id` as --id <id> to pick one.', [0, 1])

    escapeHtml(tokens, Token, parseInline)

    assert.deepEqual(shape(tokens), [
        'paragraph_open',
        'inline: Pass `--id` as --id <id> to pick one.',
        '  text: Pass `--id` as --id ',
        '  text: <id>',
        '  text:  to pick one.',
        'paragraph_close',
    ])
})

// The failure that broke the build: a wrapped line starting with `<id>` ends
// the paragraph and becomes an HTML block of its own.
test('a placeholder starting a line rejoins the paragraph it interrupted', () => {
    const tokens = [
        ...paragraph('only the text views print a sentence (now "No attachments in', [0, 1]),
        token('html_block', { content: '<id>.").\nWith fork off the list.\n', map: [1, 3] }),
    ]

    escapeHtml(tokens, Token, parseInline)

    assert.deepEqual(shape(tokens), [
        'paragraph_open',
        'inline: only the text views print a sentence (now "No attachments in\n<id>.").\nWith fork off the list.',
        '  text: only the text views print a sentence (now "No attachments in\n',
        '  text: <id>',
        '  text: .").\nWith fork off the list.',
        'paragraph_close',
    ])
    assert.deepEqual(tokens[0].map, [0, 3])
    assert.deepEqual(tokens[1].map, [0, 3])
})

test('an HTML block after a blank line becomes a paragraph of its own', () => {
    const tokens = [
        ...paragraph('First paragraph.', [0, 1]),
        token('html_block', { content: '<workspace-id> is the folder name.\n', map: [2, 3] }),
    ]

    escapeHtml(tokens, Token, parseInline)

    assert.deepEqual(shape(tokens), [
        'paragraph_open',
        'inline: First paragraph.',
        '  text: First paragraph.',
        'paragraph_close',
        'paragraph_open',
        'inline: <workspace-id> is the folder name.',
        '  text: <workspace-id>',
        '  text:  is the folder name.',
        'paragraph_close',
    ])
})

test('a comment and a self-closing component stay markup', () => {
    const tokens = [
        token('html_block', { content: '<!-- generated -->\n', map: [0, 1] }),
        token('html_block', { content: '<TicketBoard />\n', map: [2, 3] }),
        ...paragraph('Hidden <!-- note --> here.', [4, 5]),
    ]

    escapeHtml(tokens, Token, parseInline)

    assert.deepEqual(shape(tokens), [
        'html_block',
        'html_block',
        'paragraph_open',
        'inline: Hidden <!-- note --> here.',
        '  text: Hidden ',
        '  html_inline: <!-- note -->',
        '  text:  here.',
        'paragraph_close',
    ])
})

// A type name written outside a code span reads as a component, but it is
// never self-closing, so it is text too.
test('a capitalized placeholder is text', () => {
    const tokens = paragraph('Returns Vec<String> on success.', [0, 1])

    escapeHtml(tokens, Token, parseInline)

    assert.equal(tokens[1].children[1].type, 'text')
    assert.equal(tokens[1].children[1].content, '<String>')
})
