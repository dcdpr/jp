import assert from 'node:assert/strict'
import { test } from 'node:test'

import { metadataHeader } from '../site/jp-vite/meta-header.mjs'

// The parts of markdown-it's `Token` the transform uses.
class Token {
    constructor(type, tag, nesting) {
        this.type = type
        this.tag = tag
        this.nesting = nesting
        this.level = 0
        this.attrs = null
        this.content = ''
        this.children = null
        this.block = false
    }

    attrSet(name, value) {
        this.attrs = [[name, value]]
    }

    attrGet(name) {
        return this.attrs?.find(([key]) => key === name)?.[1] ?? null
    }
}

function token(type, fields = {}) {
    return Object.assign(new Token(type, fields.tag ?? '', fields.nesting ?? 0), fields)
}

function text(content) {
    return token('text', { content })
}

// The tokens of one `- **Key**: value` item, `value` given as inline tokens.
function item(key, raw, value = [text(`: ${raw}`)]) {
    return [
        token('list_item_open', { nesting: 1 }),
        token('paragraph_open', { tag: 'p', nesting: 1, level: 1 }),
        token('inline', {
            level: 2,
            content: `**${key}**: ${raw}`,
            children: [
                token('strong_open', { tag: 'strong', nesting: 1 }),
                text(key),
                token('strong_close', { tag: 'strong', nesting: -1 }),
                ...value,
            ],
        }),
        token('paragraph_close', { tag: 'p', nesting: -1, level: 1 }),
        token('list_item_close', { nesting: -1 }),
    ]
}

// A `#` title, with the link to its own anchor the site adds after a space.
function title(name) {
    const anchor = new Token('link_open', 'a', 1)
    anchor.attrs = [['class', 'header-anchor'], ['href', '#top']]
    return [
        token('heading_open', { tag: 'h1', nesting: 1 }),
        token('inline', {
            content: name,
            children: [text(name), text(' '), anchor, token('html_inline', { content: '&#8203;' }),
                new Token('link_close', 'a', -1)],
        }),
        token('heading_close', { tag: 'h1', nesting: -1 }),
    ]
}

function list(...items) {
    return [
        token('bullet_list_open', { tag: 'ul', nesting: 1 }),
        ...items.flat(),
        token('bullet_list_close', { tag: 'ul', nesting: -1 }),
    ]
}

// The tokens as the HTML they stand for, near enough to compare whole.
function shown(tokens) {
    return tokens.map(t => {
        if (t.type === 'html_block') return t.content
        if (t.type === 'paragraph_open') return `<p class="${t.attrGet('class') ?? ''}">`
        if (t.type === 'paragraph_close') return '</p>\n'
        if (t.type === 'heading_open') return `<${t.tag}>`
        if (t.type === 'heading_close') return `</${t.tag}>\n`
        if (t.type === 'bullet_list_open') return '<ul>'
        if (t.type === 'inline') {
            return t.children.map(c => {
                if (c.type === 'link_open') return `<a href="${c.attrGet('href')}">`
                if (c.type === 'link_close') return '</a>'
                return c.content
            }).join('')
        }
        return ''
    }).join('')
}

test('a wiki page keeps its title with a status letter and its labels, and ends with the rest', () => {
    const tokens = [
        ...title('Conversation'),
        ...list(
            item('Status', 'Reference'),
            item('Authors', 'Jean Mertz <git@jeanmertz.com>'),
            item('Date', '2026-05-29'),
            item('Parent', 'W-00dxybb'),
            item('Label', 'domain=conversation'),
            item('Label', 'domain=llm'),
            item('Label', 'relevant-rfd=110'),
            item('Summary', 'The words the project uses.'),
        ),
        token('heading_open', { tag: 'h2', nesting: 1 }),
        token('inline', { content: 'Turn', children: [text('Turn')] }),
        token('heading_close', { tag: 'h2', nesting: -1 }),
    ]

    metadataHeader(tokens, Token, { kind: 'wiki' })

    assert.equal(
        shown(tokens),
        '<h1>Conversation<span class="doc-status doc-status--reference" data-letter="R" ' +
            'role="img" aria-label="Reference" title="Reference: describes the system as it is, ' +
            'and changes with it"></span> <a href="#top">&#8203;</a></h1>\n' +
            '<div class="doc-meta">\n' +
            '<DocLabels kind="wiki" labels="domain%3Dconversation domain%3Dllm ' +
            'relevant-rfd%3D110" />\n' +
            '</div>\n' +
            '<h2>Turn</h2>\n' +
            '<div class="doc-meta-footer">\n' +
            '<p class="doc-meta-field"><span class="doc-meta-key">Summary</span>The words the ' +
            'project uses.</p>\n' +
            '<p class="doc-meta-field"><span class="doc-meta-key">Authors</span>Jean Mertz</p>\n' +
            '<p class="doc-meta-field"><span class="doc-meta-key">Date</span>2026-05-29</p>\n' +
            '</div>\n',
    )
})

// A hub page can be a title and a summary, and the site fills it with the pages
// under it.
test('a wiki page with no text of its own lists the pages under it', () => {
    const tokens = [
        ...title('Architecture'),
        ...list(item('Status', 'Snapshot'), item('Summary', 'Where to start.')),
    ]

    metadataHeader(tokens, Token, { kind: 'wiki' })

    assert.equal(
        shown(tokens),
        '<h1>Architecture<span class="doc-status doc-status--snapshot" data-letter="S" role="img" ' +
            'aria-label="Snapshot" title="Snapshot: dated research, not kept up to date with the ' +
            'system"></span> <a href="#top">&#8203;</a></h1>\n' +
            '<WikiChildren />\n' +
            '<div class="doc-meta-footer">\n' +
            '<p class="doc-meta-field"><span class="doc-meta-key">Summary</span>Where to start.</p>\n' +
            '</div>\n',
    )
})

test('an RFD keeps its status line and other fields, but not its summary', () => {
    const tokens = [
        ...title('RFD 042: Tool Options'),
        ...list(
            item('Status', 'In Progress'),
            item('Category', 'Design'),
            item('Authors', 'Jean Mertz <git@jeanmertz.com>'),
            item('Date', '2026-01-01'),
            item('Superseded by', 'RFD 043', [
                text(': '),
                Object.assign(new Token('link_open', 'a', 1), { attrs: [['href', './043-x.md']] }),
                text('RFD 043'),
                new Token('link_close', 'a', -1),
            ]),
            item('Promoted to', '', []),
            item('Summary', 'Tool options.'),
        ),
    ]

    metadataHeader(tokens, Token, { kind: 'rfd' })

    assert.equal(
        shown(tokens),
        '<h1>RFD 042: Tool Options <a href="#top">&#8203;</a></h1>\n' +
            '<div class="doc-meta">\n' +
            '<p class="doc-meta-line">' +
            '<span class="doc-badge doc-badge--in-progress">In Progress</span>' +
            '<span class="doc-meta-category">Design</span>' +
            '<span class="doc-meta-date">2026-01-01</span>' +
            '<span class="doc-meta-authors">Jean Mertz</span></p>\n' +
            '<p class="doc-meta-field"><span class="doc-meta-key">Superseded by</span>' +
            '<a href="./043-x.md">RFD 043</a></p>\n' +
            '</div>\n',
    )
})

// A list that isn't a metadata block, or one after a section's heading, is the
// page's own text.
test('a list that is not the metadata block is left as written', () => {
    const prose = [
        ...title('Glossary'),
        token('bullet_list_open', { tag: 'ul', nesting: 1 }),
        token('list_item_open', { nesting: 1 }),
        token('paragraph_open', { tag: 'p', nesting: 1 }),
        token('inline', { content: 'just a point', children: [text('just a point')] }),
        token('paragraph_close', { tag: 'p', nesting: -1 }),
        token('list_item_close', { nesting: -1 }),
        token('bullet_list_close', { tag: 'ul', nesting: -1 }),
    ]
    const afterSection = [
        ...title('Glossary'),
        token('heading_open', { tag: 'h2', nesting: 1 }),
        token('inline', { content: 'Fields', children: [text('Fields')] }),
        token('heading_close', { tag: 'h2', nesting: -1 }),
        ...list(item('Status', 'Reference')),
    ]

    for (const tokens of [prose, afterSection]) {
        const before = shown(tokens)
        metadataHeader(tokens, Token, { kind: 'rfd' })
        assert.equal(shown(tokens), before)
    }
})

test('text that would be HTML is escaped', () => {
    const tokens = [
        ...title('Glossary'),
        ...list(item('Category', '<script>x</script>'), item('Label', 'a=<b>')),
    ]

    metadataHeader(tokens, Token, { kind: 'rfd' })

    assert.match(shown(tokens), /&lt;script&gt;x&lt;\/script&gt;/)
    assert.match(shown(tokens), /<DocLabels kind="rfd" labels="a%3D%3Cb%3E" \/>/)
})
