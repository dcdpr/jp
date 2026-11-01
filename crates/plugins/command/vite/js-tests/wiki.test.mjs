import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    arrange,
    layout,
    pageId,
    parsePage,
    sidebarOf,
} from '../site/jp-vite/wiki.mjs'

const PAGE = `# Ubiquitous Language

- **Status**: reference
- **Authors**: jp
- **Date**: 2026-10-30
- **Parent**: w0a1b2c4
- **Label**: domain=conversation
- **Label**: relevant-rfd=064
- **Summary**: Canonical definitions for Conversation,
  Turn, and Thread.

## Turn

A single request-response cycle.
`

test('an id reads the way jp wiki reads one', () => {
    assert.equal(pageId('W-0a1b2c3'), 'W-0a1b2c3')
    assert.equal(pageId('w0A1B2C3'), 'W-0a1b2c3')
    assert.equal(pageId('0a1b2c3'), 'W-0a1b2c3')
    // `i` and `l` read as `1`, `o` as `0`.
    assert.equal(pageId('W-0a1b2ci'), 'W-0a1b2c1')
    assert.equal(pageId('Oa1b2c3'), 'W-0a1b2c3')
    // A bare id may open with `w`.
    assert.equal(pageId('wabcdef'), 'W-wabcdef')
    assert.equal(pageId('W-short'), null)
    assert.equal(pageId('RFD 042'), null)
    assert.equal(pageId(null), null)
})

test('a page reads its metadata, without case and with wrapped values', () => {
    assert.deepEqual(parsePage(PAGE, '0a1b2c3-ubiquitous-language.md'), {
        id: 'W-0a1b2c3',
        num: '0a1b2c3',
        title: 'Ubiquitous Language',
        status: 'Reference',
        parent: 'W-0a1b2c4',
        supersededBy: null,
        summary: 'Canonical definitions for Conversation, Turn, and Thread.',
        labels: ['domain=conversation', 'relevant-rfd=064'],
        slug: '0a1b2c3-ubiquitous-language',
    })
})

test('siblings come in the order file\'s order, then by id', () => {
    assert.deepEqual(
        arrange(['W-0000003', 'nope', '0000001'], ['W-0000001', 'W-0000002', 'W-0000003', 'W-0000000']),
        ['W-0000003', 'W-0000001', 'W-0000000', 'W-0000002'],
    )
})

// A page with the fields the reading order reads.
const page = (id, parent = null, status = 'Reference') =>
    ({ id, parent, status, title: id, path: `/wiki/${id.slice(2)}-x` })

const UNRANKED = { top: [], children: {} }

test('the reading order is each top page, then down its children', () => {
    const pages = [
        page('W-0000001'),
        page('W-0000002'),
        page('W-0000003', 'W-0000002'),
        page('W-0000004', 'W-0000009'),
        page('W-0000005'),
        page('W-0000006', null, 'Archived'),
        page('W-0000007', 'W-0000006'),
    ]

    const laidOut = layout(pages, { top: ['W-0000002'], children: {} })

    // A ranked top page comes first; a parent that isn't an active page leaves
    // its child at the top; an archived page is left out.
    assert.deepEqual(laidOut.map(({ depth, page }) => `${depth} ${page.id}`), [
        '0 W-0000002',
        '1 W-0000003',
        '0 W-0000001',
        '0 W-0000004',
        '0 W-0000005',
        '0 W-0000007',
    ])
})

test('pages whose parents loop are laid out once', () => {
    const pages = [page('W-0000001', 'W-0000002'), page('W-0000002', 'W-0000001')]

    const laidOut = layout(pages, UNRANKED)

    assert.deepEqual(laidOut.map(({ depth, page }) => `${depth} ${page.id}`), [
        '0 W-0000001',
        '1 W-0000002',
    ])
})

test('the sidebar holds the overview and every page, then each top page above the pages under it', () => {
    const pages = [
        page('W-0000001'),
        page('W-0000002', 'W-0000001'),
        page('W-0000004', 'W-0000002'),
        page('W-0000003'),
    ]

    assert.deepEqual(sidebarOf(pages, UNRANKED, { route: 'wiki' }), [
        { text: 'Overview', link: '/wiki/' },
        { text: 'All pages', link: '/wiki/all' },
        {
            text: 'W-0000001',
            link: '/wiki/0000001-x',
            items: [{
                text: 'W-0000002',
                link: '/wiki/0000002-x',
                items: [{ text: 'W-0000004', link: '/wiki/0000004-x' }],
            }],
        },
        { text: 'W-0000003', link: '/wiki/0000003-x' },
    ])
})
