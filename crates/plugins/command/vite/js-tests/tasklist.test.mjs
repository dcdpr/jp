import assert from 'node:assert/strict'
import { test } from 'node:test'

import { renderTaskLists } from '../site/jp-vite/tasklist.mjs'

// The parts of markdown-it's `Token` the rule uses.
class Token {
    constructor(type, tag, nesting) {
        this.type = type
        this.tag = tag
        this.nesting = nesting
        this.level = 0
        this.attrs = null
        this.content = ''
        this.children = null
    }

    attrSet(name, value) {
        const at = this.attrs?.findIndex(([key]) => key === name) ?? -1
        if (at >= 0) this.attrs[at][1] = value
        else this.attrs = [...(this.attrs ?? []), [name, value]]
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

// The tokens of one list item holding a paragraph of `children`.
function item(content, children, level = 1) {
    return [
        token('list_item_open', { tag: 'li', nesting: 1, level }),
        token('paragraph_open', { tag: 'p', nesting: 1, level: level + 1 }),
        token('inline', { content, children, level: level + 2 }),
        token('paragraph_close', { tag: 'p', nesting: -1, level: level + 1 }),
        token('list_item_close', { tag: 'li', nesting: -1, level }),
    ]
}

function list(...items) {
    return [
        token('bullet_list_open', { tag: 'ul', nesting: 1 }),
        ...items.flat(),
        token('bullet_list_close', { tag: 'ul', nesting: -1 }),
    ]
}

test('an item opening with a checkbox renders one, disabled', () => {
    const tokens = list(
        item('[ ] Open', [text('[ ] Open')]),
        item('[x] Done', [text('[x] Done')]),
    )

    renderTaskLists(tokens, Token)

    assert.equal(tokens[0].attrGet('class'), 'contains-task-list')
    assert.equal(tokens[1].attrGet('class'), 'task-list-item')
    assert.deepEqual(tokens[3].children.map(child => [child.type, child.content]), [
        ['html_inline', '<input type="checkbox" class="task-list-item-checkbox" disabled> '],
        ['text', 'Open'],
    ])
    assert.deepEqual(tokens[8].children.map(child => [child.type, child.content]), [
        ['html_inline', '<input type="checkbox" class="task-list-item-checkbox" disabled checked> '],
        ['text', 'Done'],
    ])
})

// `- [ ] **Phase**: summary` puts the checkbox in a text token of its own, so
// that token goes and the emphasis stays.
test('a checkbox ahead of emphasis leaves the emphasis whole', () => {
    const tokens = list(item('[X] **Phase**: summary', [
        text('[X] '),
        token('strong_open', { tag: 'strong', nesting: 1 }),
        text('Phase'),
        token('strong_close', { tag: 'strong', nesting: -1 }),
        text(': summary'),
    ]))

    renderTaskLists(tokens, Token)

    assert.deepEqual(tokens[3].children.map(child => child.type), [
        'html_inline', 'strong_open', 'text', 'strong_close', 'text',
    ])
    assert.match(tokens[3].children[0].content, / checked> $/)
})

test('brackets that are no checkbox, or sit outside a list item, stay text', () => {
    const tokens = [
        ...list(item('[] No space', [text('[] No space')]), item('[ ]', [text('[ ]')])),
        token('paragraph_open', { tag: 'p', nesting: 1 }),
        token('inline', { content: '[ ] Not in a list', children: [text('[ ] Not in a list')] }),
        token('paragraph_close', { tag: 'p', nesting: -1 }),
    ]

    renderTaskLists(tokens, Token)

    assert.equal(tokens[0].attrGet('class'), null)
    assert.ok(tokens.every(each => each.type !== 'inline' || each.children[0].type === 'text'))
})

// A class the list or item already carries, from `{.class}` attributes, is
// kept alongside the task-list one.
test('existing classes are kept', () => {
    const tokens = list(item('[ ] Open', [text('[ ] Open')]))
    tokens[0].attrSet('class', 'steps')

    renderTaskLists(tokens, Token)

    assert.equal(tokens[0].attrGet('class'), 'steps contains-task-list')
})
