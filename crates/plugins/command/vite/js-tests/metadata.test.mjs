import assert from 'node:assert/strict'
import { test } from 'node:test'

import { readMetadata } from '../site/jp-vite/loaders/metadata.mjs'

// An RFD header as the formatter leaves it: `Summary` is long enough to wrap,
// everything else fits on its line.
const RFD = `# RFD 042: Tool Options

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-01-01
- **Extended by**: [RFD 055]
- **Summary**: Introduces per-tool options for static user-defined
  configuration passed to tools at runtime.

## Summary

Tools gain an options field.
`

test('a document reads its title, fields, and body', () => {
    const meta = readMetadata(RFD)

    assert.equal(meta.title, 'RFD 042: Tool Options')
    assert.equal(meta.get('Status'), 'Implemented')
    assert.equal(meta.get('Extended by'), '[RFD 055]')
    assert.equal(meta.body, '## Summary\n\nTools gain an options field.')
})

test('a wrapped value is folded onto one line', () => {
    assert.equal(
        readMetadata(RFD).get('Summary'),
        'Introduces per-tool options for static user-defined configuration passed to tools at runtime.',
    )
})

test('a field stops at the next field', () => {
    // `Date` sits directly above `Extended by`, which starts a field of its
    // own and so is not a continuation of it.
    assert.equal(readMetadata(RFD).get('Date'), '2026-01-01')
})

test('a value continued on unindented lines reads whole', () => {
    // A line at the margin continues the value too, as markdown reads a list
    // item.
    const margin = `# RFD 042: Tool Options

- **Status**: Implemented
- **Summary**: Introduces per-tool options for static user-defined
configuration passed to tools at runtime.

## Summary
`

    assert.equal(
        readMetadata(margin).get('Summary'),
        'Introduces per-tool options for static user-defined configuration passed to tools at runtime.',
    )
})

test('the block ends at the blank line closing it', () => {
    const trailing = `# RFD 042: Tool Options

- **Status**: Implemented
- **Summary**: One sentence.

Body prose that is not part of the summary.
`

    const meta = readMetadata(trailing)

    assert.equal(meta.get('Summary'), 'One sentence.')
    assert.equal(meta.body, 'Body prose that is not part of the summary.')
})

test('an absent or empty field reads as null', () => {
    assert.equal(readMetadata(RFD).get('Supersedes'), null)
    assert.equal(readMetadata('# T\n\n- **Promoted to**:\n- **Status**: Done\n').get('Promoted to'), null)
})

// RFD 001 documents the metadata format in a fenced code block, so it carries
// a second, illustrative copy of every field further down the document.
const EXAMPLE = `
\`\`\`markdown
- **Status**: Draft | Discussion | Accepted
- **Summary**: One sentence, written once the RFD has a permanent number
\`\`\`
`

test('a quoted example does not shadow the header', () => {
    const quoting = readMetadata(RFD + EXAMPLE)

    assert.equal(quoting.get('Status'), 'Implemented')
    assert.equal(
        quoting.get('Summary'),
        'Introduces per-tool options for static user-defined configuration passed to tools at runtime.',
    )
})

test('a field absent from the header is null, example or no example', () => {
    // The case that matters: reading the example here would report a summary
    // the document does not have, and the build check that rejects a published
    // RFD without one would pass it.
    const bare = readMetadata(`# RFD 001: The JP RFD Process

- **Status**: Implemented
- **Category**: Process

## Summary

Prose.
${EXAMPLE}`)

    assert.equal(bare.get('Summary'), null)
    assert.equal(bare.get('Status'), 'Implemented')
})

test('a document with no metadata block reads its title and body', () => {
    const plain = readMetadata('# Title\n\nJust text.\n')

    assert.equal(plain.title, 'Title')
    assert.equal(plain.get('Status'), null)
    assert.equal(plain.body, 'Just text.')
    assert.equal(readMetadata('Just text.\n').title, null)
    assert.equal(readMetadata('Just text.\n').body, 'Just text.')
})

// `jp` reads keys without case, so the site does too.
test('a key matches without case', () => {
    const lower = readMetadata('# Page\n\n- **status**: Reference\n- **summary**: One\n  sentence.\n')

    assert.equal(lower.get('Status'), 'Reference')
    assert.equal(lower.get('Summary'), 'One sentence.')
})

// Line endings don't change what is read: `jp` reads a CRLF file as it reads
// any other.
test('a repeated field reads every value, unescaped, in order', () => {
    const page = readMetadata('# Page\r\n\r\n- **Label**: package=jp\\_config\r\n- **Status**: ' +
        'Reference\r\n- **label**: domain=llm\r\n\r\n## Labels\r\n\r\n- **Label**: not=this\r\n')

    assert.deepEqual(page.all('Label'), ['package=jp_config', 'domain=llm'])
    assert.deepEqual(page.all('Parent'), [])
    assert.equal(page.body, '## Labels\n\n- **Label**: not=this')
})
