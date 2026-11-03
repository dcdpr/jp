import assert from 'node:assert/strict'
import { test } from 'node:test'

import { ancestorsOf } from '../site/jp-vite/theme/ancestors.mjs'

// A lookup over `documents`, keyed by `id`.
const lookup = documents => {
    const byId = new Map(documents.map(document => [document.id, document]))
    return id => byId.get(id) ?? null
}

const ids = documents => documents.map(document => document.id)

const PAGES = [
    { id: 'W-0000001', parent: null },
    { id: 'W-0000003', parent: 'W-0000001' },
    { id: 'W-0000004', parent: 'W-0000003' },
]

test('a document sits below its parent, that one\'s parent, and so on, top first', () => {
    assert.deepEqual(ids(ancestorsOf('W-0000004', lookup(PAGES))), ['W-0000001', 'W-0000003'])
    assert.deepEqual(ids(ancestorsOf('W-0000001', lookup(PAGES))), [])
    assert.deepEqual(ids(ancestorsOf('W-9999999', lookup(PAGES))), [])
})

test('a loop of parents, or a parent that is not a document, still ends', () => {
    const looping = [
        { id: 'W-0000001', parent: 'W-0000002' },
        { id: 'W-0000002', parent: 'W-0000001' },
        { id: 'W-0000003', parent: 'W-0000009' },
    ]

    assert.deepEqual(ids(ancestorsOf('W-0000001', lookup(looping))), ['W-0000002'])
    assert.deepEqual(ids(ancestorsOf('W-0000003', lookup(looping))), [])
})

// The lookup decides what the trail crosses into: a ticket's trail doesn't
// climb into a wiki page, even one it names.
test('the walk stops at a document the lookup refuses', () => {
    const documents = [
        { id: 'T-0000002', kind: 'ticket', parent: 'T-0000001' },
        { id: 'T-0000001', kind: 'ticket', parent: 'W-0000001' },
        { id: 'W-0000001', kind: 'wiki', parent: null },
    ]
    const all = lookup(documents)
    const tickets = id => (all(id)?.kind === 'ticket' ? all(id) : null)

    assert.deepEqual(ids(ancestorsOf('T-0000002', tickets)), ['T-0000001'])
})
