import assert from 'node:assert/strict'
import { test } from 'node:test'

import {
    decodeLabels,
    encodeLabels,
    groupLabels,
    labelHref,
} from '../site/jp-vite/theme/label-groups.mjs'

test('labels group by key in the order each key first appears', () => {
    assert.deepEqual(
        groupLabels(['type=bug', 'package=jp_cli', 'triaged', 'package=jp_config']),
        [['type', ['bug']], ['package', ['jp_cli', 'jp_config']], ['triaged', []]],
    )
})

const SETTINGS = { ticket: { route: 'ticket' }, wiki: { route: 'knowledge' } }

test('a label leads to the listing of its kind, narrowed to it', () => {
    assert.equal(labelHref(SETTINGS, 'ticket', 'type=bug'), '/ticket/board?label=type%3Dbug')
    assert.equal(labelHref(SETTINGS, 'wiki', 'domain'), '/knowledge/all?label=domain')
    // RFDs have no listing narrowed by label, and a project may have no wiki.
    assert.equal(labelHref(SETTINGS, 'rfd', 'domain'), null)
    assert.equal(labelHref({ ticket: { route: 'ticket' } }, 'wiki', 'domain'), null)
})

// What a header passes through a static attribute has to come back as written,
// whatever it holds, and hold nothing a template would read as markup.
test('labels survive an attribute value', () => {
    const labels = ['domain=llm', 'note=a "quoted" <b> {{ x }}', 'odd=100% & more']

    const encoded = encodeLabels(labels)

    assert.doesNotMatch(encoded, /["<>{}&]/)
    assert.deepEqual(decodeLabels(encoded), labels)
    assert.deepEqual(decodeLabels(''), [])
})
