import assert from 'node:assert/strict'
import { test } from 'node:test'

import { FOLD_ABOVE, groupReferences, referenceLabel } from '../site/jp-vite/theme/reference-groups.mjs'
import { childrenOf, topPages } from '../site/jp-vite/theme/wiki-tree.mjs'

// Entries as the wiki loader gives them, in a reading order that differs from
// their ids': W-0000003 is ranked above W-0000002, and W-0000007 above
// W-0000001.
const ENTRIES = [
    { id: 'W-0000007', parent: null, depth: 0, position: 0 },
    { id: 'W-0000001', parent: null, depth: 0, position: 1 },
    { id: 'W-0000003', parent: 'W-0000001', depth: 1, position: 2 },
    { id: 'W-0000004', parent: 'W-0000003', depth: 2, position: 3 },
    { id: 'W-0000002', parent: 'W-0000001', depth: 1, position: 4 },
    // Archived: out of the reading order, though its parent is active.
    { id: 'W-0000006', parent: 'W-0000001', depth: null, position: null },
]

const ids = entries => entries.map(entry => entry.id)

test('a page lists the active pages directly under it, in reading order', () => {
    assert.deepEqual(ids(childrenOf(ENTRIES, 'W-0000001')), ['W-0000003', 'W-0000002'])
    assert.deepEqual(ids(childrenOf(ENTRIES, 'W-0000003')), ['W-0000004'])
    assert.deepEqual(ids(childrenOf(ENTRIES, 'W-0000006')), [])
    assert.deepEqual(ids(childrenOf(ENTRIES, 'W-9999999')), [])
})

test('the top pages are the active pages without a parent, in reading order', () => {
    assert.deepEqual(ids(topPages(ENTRIES)), ['W-0000007', 'W-0000001'])
})

test('references group by kind, wiki pages first, each sorted by label', () => {
    const entries = [
        { kind: 'ticket', label: 'T-0bbbbbb' },
        { kind: 'rfd', label: '097' },
        { kind: 'ticket', label: 'T-0aaaaaa' },
        { kind: 'wiki', label: 'W-0000001' },
        { kind: 'rfd', label: 'D50' },
    ]

    assert.deepEqual(
        groupReferences(entries).map(({ kind, name, entries }) => [kind, name, entries.map(referenceLabel)]),
        [
            ['wiki', 'Wiki pages', ['W-0000001']],
            ['rfd', 'RFDs', ['RFD 097', 'RFD D50']],
            ['ticket', 'Tickets', ['T-0aaaaaa', 'T-0bbbbbb']],
        ],
    )
    assert.deepEqual(groupReferences([]), [])
    assert.equal(FOLD_ABOVE, 6)
})
