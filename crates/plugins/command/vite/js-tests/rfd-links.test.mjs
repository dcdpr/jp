import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    canonicalRfd,
    findBrokenRfdLinks,
    mergeTickets,
    rfdTickets,
} from '../site/jp-vite/loaders/rfd-shared.mjs'

// The settings `jp vite sync` writes for a project with both workflows and the
// default link keys.
const SETTINGS = {
    rfd: { route: 'rfd', trackingKey: 'tracking-rfd', relevantKey: 'relevant-rfd' },
    ticket: { route: 'ticket' },
}

const ticket = (id, status, labels) => ({ id, status, labels })

test('an id reads the way jp rfd reads one', () => {
    assert.equal(canonicalRfd('116'), '116')
    assert.equal(canonicalRfd('91'), '091')
    assert.equal(canonicalRfd('RFD 7'), '007')
    assert.equal(canonicalRfd('d5'), 'D05')
    assert.equal(canonicalRfd('whatever'), null)
})

test('an RFD is in development while a ticket tracking it is In Progress', () => {
    const tickets = [
        ticket('T-02wt0k1', 'In Progress', ['tracking-rfd=116', 'type=tracking']),
        ticket('T-02wt0k2', 'In Progress', ['relevant-rfd=117']),
        ticket('T-02wt0k3', 'Todo', ['tracking-rfd=118']),
        ticket('T-02wt0k4', 'Done', ['relevant-rfd=116']),
    ]

    const byRfd = rfdTickets(tickets, SETTINGS)

    assert.deepEqual([...byRfd.keys()], ['116', '117', '118'])
    assert.equal(byRfd.get('116').inDevelopment, true)
    assert.equal(byRfd.get('117').inDevelopment, false)
    assert.equal(byRfd.get('118').inDevelopment, false)
    assert.deepEqual([...byRfd.get('116').tickets], ['T-02wt0k1', 'T-02wt0k4'])
})

test('the keys are the ones jp rfd is configured with', () => {
    const settings = {
        ...SETTINGS,
        rfd: { ...SETTINGS.rfd, trackingKey: 'tracks', relevantKey: 'rfd' },
    }

    const byRfd = rfdTickets(
        [ticket('T-02wt0k1', 'In Progress', ['tracking-rfd=117', 'tracks=116'])],
        settings,
    )

    assert.deepEqual([...byRfd.keys()], ['116'])
    assert.equal(byRfd.get('116').inDevelopment, true)
})

test('each RFD links to the tickets naming it on the board, by their labels', () => {
    const entries = [{ num: '116' }, { num: '117' }]

    mergeTickets(entries, [ticket('T-02wt0k1', 'In Progress', ['tracking-rfd=116'])], SETTINGS)

    assert.deepEqual(entries[0], {
        num: '116',
        inDevelopment: true,
        ticketCount: 1,
        ticketsPath: '/ticket/board?any-label=tracking-rfd%3D116&any-label=relevant-rfd%3D116',
    })
    assert.equal(entries[1].inDevelopment, false)
    assert.equal(entries[1].ticketCount, 0)
})

test('a project without tickets links nowhere', () => {
    const entries = [{ num: '116' }]

    mergeTickets(entries, [], { rfd: SETTINGS.rfd, ticket: null })

    assert.equal(entries[0].ticketsPath, null)
})

// `relevant-rfd=91` names RFD 091 to `jp rfd`, but not to the board's filter,
// which compares labels as written.
test('a link naming no RFD, or one spelled another way, is reported', () => {
    const known = new Set(['091', '116'])
    const tickets = [
        ticket('T-02wt0k1', 'Todo', ['relevant-rfd=091', 'tracking-rfd=116']),
        ticket('T-02wt0k2', 'Todo', [
            'relevant-rfd=91',
            'relevant-rfd=1160',
            'tracking-rfd',
            'type=bug',
        ]),
    ]

    assert.equal(
        findBrokenRfdLinks(tickets, known, SETTINGS),
        'Tickets link to RFDs that don\'t exist, or by an id spelled another way:\n'
            + '  T-02wt0k2: relevant-rfd=91\n'
            + '  T-02wt0k2: relevant-rfd=1160\n'
            + '  T-02wt0k2: tracking-rfd\n\n'
            + 'Write the RFD\'s id the way `jp rfd list` shows it, such as `042` or `D05`.',
    )
    assert.equal(findBrokenRfdLinks(tickets.slice(0, 1), known, SETTINGS), null)
})
