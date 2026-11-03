import assert from 'node:assert/strict'
import { test } from 'node:test'

import { findBrokenParents, parseTicket } from '../site/jp-vite/loaders/ticket-shared.mjs'
import {
    childrenOf,
    inGroup,
    parseParentFilter,
    progress,
    withParentFilter,
} from '../site/jp-vite/theme/ticket-tree.mjs'

const ticket = (id, status, parent = null) => ({ id, status, parent })

// A parent with two children, one of them Done, and a ticket under nothing.
const BOARD = [
    ticket('T-02wt0k1', 'Todo'),
    ticket('T-02wt0k2', 'Todo', 'T-02wt0k1'),
    ticket('T-02wt0k3', 'Done', 'T-02wt0k1'),
    ticket('T-02wt0k4', 'Todo'),
]

test('a ticket file names its parent', () => {
    const parsed = parseTicket(
        '# Bound tool output\n\n- **Status**: Todo\n- **Authors**: jp\n- **Date**: 2026-11-02\n' +
            '- **Parent**: T-02wt0k1\n\nBody.\n',
        '02wt0k2-bound-tool-output.md',
    )

    assert.equal(parsed.parent, 'T-02wt0k1')
})

test('a ticket file without a parent names none', () => {
    const parsed = parseTicket(
        '# Alone\n\n- **Status**: Todo\n- **Authors**: jp\n- **Date**: 2026-11-02\n',
        '02wt0k4-alone.md',
    )

    assert.equal(parsed.parent, null)
})

test('children are the tickets naming the parent, in the order given', () => {
    assert.deepEqual(childrenOf(BOARD, 'T-02wt0k1').map(each => each.id), ['T-02wt0k2', 'T-02wt0k3'])
    assert.deepEqual(childrenOf(BOARD, 'T-02wt0k4'), [])
})

test('progress counts the done children', () => {
    assert.deepEqual(progress(childrenOf(BOARD, 'T-02wt0k1')), { done: 1, total: 2 })
})

test('a group is the parent and its children', () => {
    const shown = BOARD.filter(each => inGroup(each, 'T-02wt0k1')).map(each => each.id)

    assert.deepEqual(shown, ['T-02wt0k1', 'T-02wt0k2', 'T-02wt0k3'])
    assert.equal(BOARD.every(each => inGroup(each, null)), true)
})

test('the parent filter reads and writes `under`, keeping the other parameters', () => {
    assert.equal(parseParentFilter('?label=type=bug&under=T-02wt0k1'), 'T-02wt0k1')
    assert.equal(parseParentFilter('?label=type=bug'), null)
    assert.equal(parseParentFilter('?under='), null)

    assert.equal(withParentFilter('?label=type%3Dbug', 'T-02wt0k1'), '?label=type%3Dbug&under=T-02wt0k1')
    assert.equal(withParentFilter('?under=T-02wt0k1', null), '')
})

test('a board keeping every rule of the parent relation has nothing to report', () => {
    assert.equal(findBrokenParents(BOARD), null)
})

test('every broken parent relation is named', () => {
    const tickets = [
        ticket('T-02wt0k1', 'Done'),
        ticket('T-02wt0k2', 'Todo', 'T-02wt0k1'),
        ticket('T-02wt0k3', 'Todo', 'T-02wt0k2'),
        ticket('T-02wt0k4', 'Todo', 'T-0zzzzzz'),
        ticket('T-02wt0k5', 'Todo', 'T-02wt0k5'),
    ]

    assert.equal(
        findBrokenParents(tickets),
        'Tickets break the rules of the parent relation:\n' +
            '  T-02wt0k1: is Done with open children T-02wt0k2\n' +
            '  T-02wt0k3: sits under T-02wt0k2, which sits under T-02wt0k1\n' +
            '  T-02wt0k4: names T-0zzzzzz as its parent, which is not a ticket\n' +
            '  T-02wt0k5: is its own parent\n\n' +
            'A ticket sits under one that exists and sits under nothing, and a parent closes ' +
            'once its children have. `jp ticket check` says the same, and `jp ticket edit ' +
            '--parent` and `--status` fix it.',
    )
})
