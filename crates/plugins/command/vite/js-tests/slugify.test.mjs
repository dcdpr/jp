import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import { slugify } from '../site/jp-vite/slugify.mjs'

// The cases `doc::heading` is tested against in Rust, which is what `jp wiki
// check` holds a `#fragment` to: each heading's text, as the site reads it, and
// the slug it takes.
const CASES = JSON.parse(readFileSync(
    new URL('../../../../internal/doc/fixtures/heading-slugs.json', import.meta.url),
    'utf-8',
))

// How markdown-it-anchor tells apart two headings with one slug: the later one
// takes the first of `-1`, `-2`, and so on that no heading took yet.
function claim(slug, taken) {
    let unique = slug
    let suffix = 1
    while (taken.has(unique)) unique = `${slug}-${suffix++}`
    taken.add(unique)

    return unique
}

for (const { case: name, texts, slugs } of CASES) {
    test(`the site's slugs match Rust's: ${name}`, () => {
        const taken = new Set()

        assert.deepEqual(texts.map(text => claim(slugify(text), taken)), slugs)
    })
}

test('a slug of a slug is the slug', () => {
    for (const slug of ['pinned-conversation', '_1-query-command', 'examples-1', '日本語-heading']) {
        assert.equal(slugify(slug), slug)
    }
})
