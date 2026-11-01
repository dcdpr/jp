import assert from 'node:assert/strict'
import { test } from 'node:test'

import { ticketCommands, wikiCommands } from '../site/jp-vite/middleware.mjs'

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

test('a new wiki page names its title, and what else it was given', () => {
    assert.deepEqual(wikiCommands({
        action: 'add',
        title: 'Terms',
        status: '',
        summary: 'The words.',
        parent: 'W-0a1b2c3',
        body: '## Turn',
        labels: ['domain=llm'],
    }), {
        commands: [['wiki', 'add', 'Terms', '--summary=The words.', '--parent=W-0a1b2c3',
            '--body=## Turn', '--label=domain=llm']],
    })
    assert.deepEqual(wikiCommands({ action: 'add', title: '' }), {
        refusal: 'add needs a title',
    })
})

// An empty parent is the form's "top page", which `jp wiki edit` spells `none`.
test('a wiki edit moves a page under another, or to the top', () => {
    assert.deepEqual(wikiCommands({ action: 'edit', id: 'W-0a1b2c3', parent: 'W-0b42fs5' }), {
        commands: [['wiki', 'edit', 'W-0a1b2c3', '--parent=W-0b42fs5']],
    })
    assert.deepEqual(wikiCommands({ action: 'edit', id: 'W-0a1b2c3', parent: '' }), {
        commands: [['wiki', 'edit', 'W-0a1b2c3', '--parent=none']],
    })
})

// An edit adds and removes the difference, so a label nobody touched stays
// written as it was.
test('a wiki edit changes the labels by their difference', () => {
    assert.deepEqual(wikiCommands({
        action: 'edit',
        id: '0a1b2c3',
        summary: 'Fewer words.',
        labels: ['domain=llm', 'domain=storage'],
        labelsBefore: ['domain=llm', 'type=note'],
    }), {
        commands: [['wiki', 'edit', 'W-0a1b2c3', '--summary=Fewer words.',
            '--remove-label=type=note', '--add-label=domain=storage']],
    })
})

// A value joined to its flag can open with `-`: a section is often a list.
test('a wiki section edit replaces that section, whatever its text opens with', () => {
    assert.deepEqual(wikiCommands({
        action: 'edit',
        id: 'W-0a1b2c3',
        section: 'turn',
        body: '- one\n- two',
    }), {
        commands: [['wiki', 'edit', 'W-0a1b2c3', '--body=- one\n- two', '--section=turn']],
    })
    // An empty body empties the section rather than asking for an editor.
    assert.deepEqual(wikiCommands({ action: 'edit', id: 'W-0a1b2c3', section: 'turn', body: '' }), {
        commands: [['wiki', 'edit', 'W-0a1b2c3', '--body=', '--section=turn']],
    })
    assert.deepEqual(wikiCommands({ action: 'edit', id: 'W-0a1b2c3', section: 'turn' }), {
        refusal: 'a section edit needs a body',
    })
})

// With nothing to change, `jp wiki edit` would open an editor nobody sees.
test('a wiki edit that changes nothing runs nothing', () => {
    assert.deepEqual(wikiCommands({ action: 'edit', id: 'W-0a1b2c3' }), { commands: [] })
})

test('a wiki page is archived, never deleted', () => {
    assert.deepEqual(wikiCommands({ action: 'archive', id: 'w-0a1b2c3' }), {
        commands: [['wiki', 'archive', 'W-0a1b2c3']],
    })
    assert.deepEqual(wikiCommands({ action: 'delete', id: 'W-0a1b2c3' }), {
        refusal: 'expected action: add, edit, or archive',
    })
})
