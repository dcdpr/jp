import assert from 'node:assert/strict'
import { test } from 'node:test'

import { repositoryLinkFor } from '../site/jp-vite/config.mjs'

// A site in `docs`, linking repository files on GitHub.
const SETTINGS = { site: 'docs', repository: 'https://github.com/dcdpr/jp' }

test('a link out of the site leads to the file in the repository', () => {
    assert.equal(
        repositoryLinkFor('../../crates/jp_cli/src/lib.rs', 'docs/wiki', SETTINGS),
        'https://github.com/dcdpr/jp/blob/HEAD/crates/jp_cli/src/lib.rs',
    )
    assert.equal(
        repositoryLinkFor('../../crates/jp_cli', 'docs/wiki', SETTINGS),
        'https://github.com/dcdpr/jp/tree/HEAD/crates/jp_cli',
    )
    assert.equal(repositoryLinkFor('./0a1b2c3-glossary.md', 'docs/wiki', SETTINGS), null)
})

// `jp wiki check` reads four forms of line range; a forge reads two.
test('lines are linked the way a forge reads them', () => {
    const base = 'https://github.com/dcdpr/jp/blob/HEAD/crates/x/src/lib.rs'
    const link = fragment =>
        repositoryLinkFor(`../../crates/x/src/lib.rs#${fragment}`, 'docs/wiki', SETTINGS)

    assert.equal(link('L12'), `${base}#L12`)
    assert.equal(link('L12-L24'), `${base}#L12-L24`)
    assert.equal(link('L12-24'), `${base}#L12-L24`)
    assert.equal(link('12-24'), `${base}#L12-L24`)
    assert.equal(link('by_rfd'), `${base}#by_rfd`)
})
