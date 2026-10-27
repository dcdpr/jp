import assert from 'node:assert/strict'
import { test } from 'node:test'

import { findMiscountedLabels } from '../site/jp-vite/loaders/ticket-shared.mjs'

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
