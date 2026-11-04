import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

import { redirectFor, siteRedirects } from '../site/jp-vite/redirects.mjs'

// A site with an RFD, its template, a draft, a ticket, and a wiki page, each
// workflow's index pages beside them.
function site() {
    const dir = mkdtempSync(join(tmpdir(), 'jp-vite-redirects-'))
    mkdirSync(join(dir, 'rfd/drafts'), { recursive: true })
    mkdirSync(join(dir, 'ticket'))
    mkdirSync(join(dir, 'wiki'))
    for (const file of [
        'rfd/000-design-template.md',
        'rfd/101-conversation-labels.md',
        'rfd/index.md',
        'rfd/drafts/D12-knowledge-base.md',
        'ticket/005zd00-prepare-ticket-board.md',
        'ticket/board.md',
        'ticket/index.md',
        'wiki/0a1b2c3-glossary.md',
        'wiki/index.md',
    ]) {
        writeFileSync(join(dir, file), '# A document\n')
    }

    return dir
}

const SETTINGS = { rfd: { route: 'rfd' }, ticket: { route: 'ticket' }, wiki: { route: 'wiki' } }

// Under its workflow's route by id, and at the site root by its prefixed id.
// Neither the template nor the draft gets one.
test('an RFD, a ticket, and a wiki page are reached by their ids', () => {
    assert.deepEqual(Object.fromEntries(siteRedirects(site(), SETTINGS)), {
        '/rfd/101': '/rfd/101-conversation-labels',
        '/rfd-101': '/rfd/101-conversation-labels',
        '/ticket/005zd00': '/ticket/005zd00-prepare-ticket-board',
        '/ticket/T-005zd00': '/ticket/005zd00-prepare-ticket-board',
        '/T-005zd00': '/ticket/005zd00-prepare-ticket-board',
        '/wiki/0a1b2c3': '/wiki/0a1b2c3-glossary',
        '/wiki/W-0a1b2c3': '/wiki/0a1b2c3-glossary',
        '/W-0a1b2c3': '/wiki/0a1b2c3-glossary',
    })
})

// The root short link of an RFD follows the route the project serves RFDs on.
test('an RFD\'s root short link follows its route', () => {
    const dir = mkdtempSync(join(tmpdir(), 'jp-vite-redirects-'))
    mkdirSync(join(dir, 'design'))
    writeFileSync(join(dir, 'design/007-a-design.md'), '# A document\n')

    assert.deepEqual(Object.fromEntries(siteRedirects(dir, { rfd: { route: 'design' } })), {
        '/design/007': '/design/007-a-design',
        '/design-007': '/design/007-a-design',
    })
})

// A redirect naming a wiki page by id keeps working when the page is renamed.
test('a moved page lands on the route, or the wiki page, it maps to', () => {
    const redirects = siteRedirects(site(), {
        ...SETTINGS,
        redirects: {
            '/architecture/glossary': 'W-0a1b2c3',
            '/architecture/': '/wiki/',
        },
    })

    assert.equal(redirects.get('/architecture/glossary'), '/wiki/0a1b2c3-glossary')
    assert.equal(redirects.get('/architecture/'), '/wiki/')
})

test('a moved page naming a wiki page that isn\'t there is an error', () => {
    assert.throws(
        () => siteRedirects(site(), { ...SETTINGS, redirects: { '/old': 'W-9999999' } }),
        /sends `\/old` to W-9999999, and no wiki page has that id/,
    )
})

test('a project without the workflows has no short links', () => {
    assert.equal(siteRedirects(site(), {}).size, 0)
})

// A page and the directory beside it can both have moved: `/old` and `/old/`
// are different routes.
test('the dev server matches a route as asked for before ignoring its slash', () => {
    const redirects = new Map([
        ['/old', '/wiki/0aaaaaa-page'],
        ['/old/', '/wiki/0bbbbbb-index'],
        ['/rfd/101', '/rfd/101-labels'],
    ])

    assert.equal(redirectFor(redirects, '/old'), '/wiki/0aaaaaa-page')
    assert.equal(redirectFor(redirects, '/old/'), '/wiki/0bbbbbb-index')
    assert.equal(redirectFor(redirects, '/rfd/101/'), '/rfd/101-labels')
    assert.equal(redirectFor(redirects, '/elsewhere'), undefined)
})
