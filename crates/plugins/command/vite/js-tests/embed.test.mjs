import assert from 'node:assert/strict'
import { test } from 'node:test'

import { classify, lineRange, rebase } from '../site/jp-vite/embed.mjs'

const SETTINGS = { site: 'docs' }

test('a line range reads the way jp wiki reads one', () => {
    assert.deepEqual(lineRange('L12'), [12, 12])
    assert.deepEqual(lineRange('L12-L24'), [12, 24])
    assert.deepEqual(lineRange('L12-24'), [12, 24])
    assert.deepEqual(lineRange('12-24'), [12, 24])
    assert.equal(lineRange('turn'), null)
    assert.equal(lineRange('L12-'), null)
    assert.equal(lineRange('12'), null)
})

// A section is shown on another page, so its links have to lead where they did
// on its own.
test('a link in an embedded section reads the same from the embedding page', () => {
    const from = 'wiki/0a1b2c3-glossary.md'

    // A fragment alone names the source page, not the embedding one.
    assert.equal(rebase('#turn', from, 'wiki/0b0b0b0-page.md'), './0a1b2c3-glossary.md#turn')
    assert.equal(rebase('#turn', from, 'rfd/042-x.md'), '../wiki/0a1b2c3-glossary.md#turn')
    assert.equal(rebase('../rfd/100-x.md#labels', from, 'rfd/042-x.md'), './100-x.md#labels')
    assert.equal(rebase('./0aaaaaa-a.md', from, 'guide/setup.md'), '../wiki/0aaaaaa-a.md')
    // Addresses that read the same anywhere stay as they are.
    assert.equal(rebase('https://example.com/x', from, 'rfd/042-x.md'), 'https://example.com/x')
    assert.equal(rebase('/wiki/', from, 'rfd/042-x.md'), '/wiki/')
    assert.equal(rebase('mailto:a@b.c', from, 'rfd/042-x.md'), 'mailto:a@b.c')
})

test('an image of a document is an embed of one of its sections', () => {
    assert.deepEqual(classify('0a1b2c3-glossary.md#pinned-conversation', 'wiki/0b0b0b0-page.md', SETTINGS), {
        kind: 'section',
        target: 'wiki/0a1b2c3-glossary.md',
        slug: 'pinned-conversation',
    })
    assert.deepEqual(classify('0a1b2c3-glossary.md', 'wiki/0b0b0b0-page.md', SETTINGS), {
        problem: 'names a document but no section of it: add one, as `#turn`.',
    })
})

test('an image of any other file with a line range is an embed of those lines', () => {
    assert.deepEqual(classify('../../crates/x/src/lib.rs#L2-4', 'wiki/0b0b0b0-page.md', SETTINGS), {
        kind: 'lines',
        target: '../crates/x/src/lib.rs',
        start: 2,
        end: 4,
    })
})

test('an image of an image is no embed, and neither is one elsewhere', () => {
    assert.equal(classify('diagram.svg', 'wiki/0b0b0b0-page.md', SETTINGS), null)
    assert.equal(classify('../../../logo.png', 'wiki/0b0b0b0-page.md', SETTINGS), null)
    assert.equal(classify('https://example.com/x.md#y', 'wiki/0b0b0b0-page.md', SETTINGS), null)
    assert.equal(classify('/logo.png', 'wiki/0b0b0b0-page.md', SETTINGS), null)
})

test('an image of a file that is no image, without a line range, is refused', () => {
    // The site files sit two levels below what they read as the source
    // directory, as `.vitepress/jp-vite/` does in a project.
    assert.deepEqual(classify('site/jp-vite/config.mjs', 'index.md', SETTINGS), {
        problem: 'names a file that is no image: embed a line range of it, as `#L12-L24`.',
    })
})

test('an embed reaches only the workspace', () => {
    assert.deepEqual(classify('../../../etc/passwd#L1', 'wiki/0b0b0b0-page.md', SETTINGS), {
        problem: 'points outside the workspace.',
    })
})
