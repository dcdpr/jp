import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

import { redirectFor, siteRedirects } from '../site/jp-vite/redirects.mjs'

// A site with an RFD, its template, and a wiki page.
function site() {
    const dir = mkdtempSync(join(tmpdir(), 'jp-vite-redirects-'))
    mkdirSync(join(dir, 'rfd'))
    mkdirSync(join(dir, 'wiki'))
    for (const file of [
        'rfd/000-design-template.md',
        'rfd/101-conversation-labels.md',
        'wiki/0a1b2c3-glossary.md',
        'wiki/index.md',
    ]) {
        writeFileSync(join(dir, file), '# A document\n')
    }

    return dir
}

const SETTINGS = { rfd: { route: 'rfd' }, wiki: { route: 'wiki' } }

test('an RFD and a wiki page are reached by their ids', () => {
    assert.deepEqual(Object.fromEntries(siteRedirects(site(), SETTINGS)), {
        '/rfd/101': '/rfd/101-conversation-labels',
        '/wiki/0a1b2c3': '/wiki/0a1b2c3-glossary',
        '/wiki/W-0a1b2c3': '/wiki/0a1b2c3-glossary',
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
