import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    findMiscountedLabels,
    findSharedLabels,
    findUnknownLabels,
} from '../site/jp-vite/loaders/ticket-shared.mjs'

// A board where every ticket carries exactly one `type`, and at most one
// `team`.
const VOCABULARY = {
    type: { description: 'The type', values: ['bug', 'feature'], min: 1, max: 1 },
    team: { description: 'The team', values: ['core', 'web'], max: 1 },
    area: { description: 'The area', values: ['cli', 'docs'] },
}

const ticket = (id, labels) => ({ id, labels })

test('a board with no bounded key has nothing to report', () => {
    const tickets = [ticket('T-02wt0k1', [])]

    assert.equal(findMiscountedLabels(tickets, {}), null)
    assert.equal(findMiscountedLabels(tickets, { area: VOCABULARY.area }), null)
})

test('tickets meeting every bound pass', () => {
    const tickets = [
        ticket('T-02wt0k1', ['type=bug']),
        ticket('T-02wt0k2', ['area=cli', 'area=docs', 'team=web', 'type=feature']),
    ]

    assert.equal(findMiscountedLabels(tickets, VOCABULARY), null)
})

test('each ticket names the keys it carries too few or too many of', () => {
    const tickets = [
        ticket('T-02wt0k1', ['area=cli']),
        ticket('T-02wt0k2', ['team=core', 'team=web', 'type=bug', 'type=feature']),
        ticket('T-02wt0k3', ['type=bug']),
    ]

    assert.equal(
        findMiscountedLabels(tickets, VOCABULARY),
        'Tickets carry too few or too many labels of one key:\n'
            + '  T-02wt0k1: 0 `type` (needs exactly 1)\n'
            + '  T-02wt0k2: 2 `type` (needs exactly 1), 2 `team` (needs at most 1)\n\n'
            + 'Fix the tickets (`jp ticket label <id> --label key=value`), or change the key\'s '
            + '`min` and `max` in `plugins.command.ticket.options.labels`.',
    )
})

// A key that takes any value is written `values: []` and marked `open`; a bare
// key has neither values nor the mark.
const OPEN = {
    rfd: { description: 'The RFD', values: [], open: true, unique: 'key-value' },
    focus: { description: 'Being worked on now', values: [], unique: 'key' },
    owner: { description: 'Who', values: ['jean', 'jp'], unique: 'key' },
    area: { description: 'The area', values: ['cli'] },
}

test('a key taking any value accepts every value, but not the key alone', () => {
    const tickets = [
        ticket('T-02wt0k1', ['rfd=116', 'rfd=Some title', 'focus', 'area=cli']),
        ticket('T-02wt0k2', ['rfd', 'rfd=', 'area=docs']),
    ]

    assert.equal(
        findUnknownLabels(tickets, OPEN),
        'Tickets carry labels the vocabulary doesn\'t define:\n'
            + '  T-02wt0k2: rfd, rfd=, area=docs\n\n'
            + 'Add them to `plugins.command.ticket.options.labels`, or fix the tickets '
            + '(`jp ticket label <id> --label key=value`).',
    )
})

test('labels the board lets one ticket carry pass when each is on one', () => {
    const tickets = [
        ticket('T-02wt0k1', ['rfd=116', 'focus', 'owner=jean']),
        ticket('T-02wt0k2', ['rfd=117', 'area=cli']),
    ]

    assert.equal(findSharedLabels(tickets, OPEN), null)
    assert.equal(findSharedLabels(tickets, { area: OPEN.area }), null)
})

// `key-value` holds each value to one ticket, matching without case; `key`
// holds the whole key, whatever value each ticket carries.
test('a label on several tickets names them all', () => {
    const tickets = [
        ticket('T-02wt0k1', ['rfd=abc', 'focus', 'owner=jean']),
        ticket('T-02wt0k2', ['rfd=ABC', 'rfd=117', 'owner=jp']),
        ticket('T-02wt0k3', ['focus', 'rfd=118']),
    ]

    assert.equal(
        findSharedLabels(tickets, OPEN),
        'Labels the board lets one ticket carry are on several:\n'
            + '  rfd=abc: T-02wt0k1, T-02wt0k2\n'
            + '  focus: T-02wt0k1, T-02wt0k3\n'
            + '  owner: T-02wt0k1, T-02wt0k2\n\n'
            + 'Keep each on one ticket (`jp ticket label <id> --label key=value`), or drop '
            + '`unique` from the key in `plugins.command.ticket.options.labels`.',
    )
})

// `typed=x` shares a prefix with `type`, and must not count toward it.
test('only labels under the key itself count', () => {
    const vocabulary = { type: VOCABULARY.type, typed: { values: ['x'] } }

    assert.equal(
        findMiscountedLabels([ticket('T-02wt0k1', ['typed=x'])], vocabulary),
        'Tickets carry too few or too many labels of one key:\n'
            + '  T-02wt0k1: 0 `type` (needs exactly 1)\n\n'
            + 'Fix the tickets (`jp ticket label <id> --label key=value`), or change the key\'s '
            + '`min` and `max` in `plugins.command.ticket.options.labels`.',
    )
})
