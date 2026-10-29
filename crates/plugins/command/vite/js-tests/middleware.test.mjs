import assert from 'node:assert/strict'
import { test } from 'node:test'

import { ticketCommands } from '../site/jp-vite/middleware.mjs'

const edit = (fields) => ticketCommands({ action: 'edit', id: 'T-02wt0kx', ...fields })

test('an edit that leaves the labels out leaves them alone', () => {
    assert.deepEqual(edit({ title: 'New title', status: 'Todo' }), {
        commands: [['ticket', 'edit', 'T-02wt0kx', '--title', 'New title', '--status', 'Todo']],
    })
})

test('an edit naming labels sets exactly those', () => {
    assert.deepEqual(edit({ status: 'Todo', labels: ['client=cli', 'type=bug'] }), {
        commands: [['ticket', 'edit', 'T-02wt0kx', '--status', 'Todo',
            '--label', 'client=cli', '--label', 'type=bug']],
    })
})

// `edit` without `--label` keeps the labels, so clearing them has to be a
// command of its own, and it runs before the rest of the edit.
test('an edit with an empty label list takes every label off first', () => {
    assert.deepEqual(edit({ title: 'New title', status: 'Done', labels: [] }), {
        commands: [
            ['ticket', 'label', 'rm', '--id', 'T-02wt0kx'],
            ['ticket', 'edit', 'T-02wt0kx', '--title', 'New title', '--status', 'Done'],
        ],
    })
})

test('an edit that only clears the labels runs nothing else', () => {
    assert.deepEqual(edit({ labels: [''] }), {
        commands: [['ticket', 'label', 'rm', '--id', 'T-02wt0kx']],
    })
})

// With nothing to change, `jp ticket edit` would open an editor nobody sees.
test('an edit that changes nothing runs nothing', () => {
    assert.deepEqual(edit({}), { commands: [] })
})

test('add needs a title, and an unknown action is refused', () => {
    assert.deepEqual(ticketCommands({ action: 'add', title: '' }), { refusal: 'add needs a title' })
    assert.deepEqual(ticketCommands({ action: 'frobnicate' }), {
        refusal: 'expected action: add, edit, or delete',
    })
})
