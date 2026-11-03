import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    anyLabelQuery,
    describeLabelFilter,
    hasLabelTerm,
    matchesLabelFilter,
    parseLabelFilter,
    toggleLabelTerm,
    withLabelFilter,
} from '../site/jp-vite/theme/label-filter.mjs'

test('a query without label terms has no filter', () => {
    assert.equal(parseLabelFilter(''), null)
    assert.equal(parseLabelFilter('?other=1'), null)
    assert.equal(parseLabelFilter('?label=%20'), null)
})

test('every label term has to match, and one any-label term', () => {
    const filter = parseLabelFilter(
        '?label=type%3Dbug&label=client&any-label=rfd%3D116&any-label=rfd%3D117',
    )
    assert.deepEqual(filter, { all: ['type=bug', 'client'], any: ['rfd=116', 'rfd=117'] })

    assert.equal(matchesLabelFilter(['client=cli', 'rfd=117', 'type=bug'], filter), true)
    // Nothing under `client`.
    assert.equal(matchesLabelFilter(['rfd=117', 'type=bug'], filter), false)
    // Neither of the RFDs.
    assert.equal(matchesLabelFilter(['client=cli', 'rfd=118', 'type=bug'], filter), false)
})

// `clientele` starts with `client`, and isn't the key.
test('a term matches without case, and a bare key any value under it', () => {
    const filter = parseLabelFilter('?label=Type%3DBug&label=CLIENT')

    assert.equal(matchesLabelFilter(['client=cli', 'type=bug'], filter), true)
    assert.equal(matchesLabelFilter(['client', 'type=bug'], filter), true)
    assert.equal(matchesLabelFilter(['clientele=x', 'type=bug'], filter), false)
})

test('no filter lets every ticket through', () => {
    assert.equal(matchesLabelFilter([], null), true)
})

test('the banner names the filter', () => {
    assert.equal(
        describeLabelFilter({ all: ['type=bug'], any: ['rfd=116', 'rfd=117'] }),
        'Tickets labelled type=bug, and rfd=116 or rfd=117',
    )
    assert.equal(describeLabelFilter({ all: [], any: ['rfd=116'] }), 'Tickets labelled rfd=116')
    assert.equal(describeLabelFilter({ all: ['domain'], any: [] }, 'Pages'), 'Pages labelled domain')
})

// A key clicked on the board asks for any value under it, a value for that
// label, and a second click on either takes it back out.
test('a clicked label narrows the filter, and a second click widens it back', () => {
    const byKey = toggleLabelTerm(null, 'domain')
    assert.deepEqual(byKey, { all: ['domain'], any: [] })

    const both = toggleLabelTerm(byKey, 'type=bug')
    assert.deepEqual(both, { all: ['domain', 'type=bug'], any: [] })
    assert.equal(hasLabelTerm(both, 'TYPE=bug'), true)
    assert.equal(hasLabelTerm(both, 'type'), false)

    assert.deepEqual(toggleLabelTerm(both, 'Domain'), { all: ['type=bug'], any: [] })
    assert.equal(toggleLabelTerm(byKey, 'domain'), null)

    // A link's `any-label` terms stay while labels are clicked on and off.
    const linked = { all: [], any: ['rfd=116'] }
    assert.deepEqual(toggleLabelTerm(toggleLabelTerm(linked, 'domain'), 'domain'), linked)
})

// A table keeps its own search and filter in the address beside the labels.
test('a filter written into a query reads back, and keeps what else it holds', () => {
    const filter = { all: ['domain', 'type=bug'], any: ['rfd=116'] }

    const query = withLabelFilter('?search=turn&label=old', filter)

    assert.equal(query, '?search=turn&label=domain&label=type%3Dbug&any-label=rfd%3D116')
    assert.deepEqual(parseLabelFilter(query), filter)
    assert.equal(withLabelFilter('?search=turn&label=old', null), '?search=turn')
    assert.equal(withLabelFilter('?label=old', null), '')
})

test('a link to the tickets carrying any of some labels reads back as that filter', () => {
    const query = anyLabelQuery(['tracking-rfd=116', 'relevant-rfd=116'])

    assert.equal(query, '?any-label=tracking-rfd%3D116&any-label=relevant-rfd%3D116')
    assert.deepEqual(parseLabelFilter(query), {
        all: [],
        any: ['tracking-rfd=116', 'relevant-rfd=116'],
    })
})
