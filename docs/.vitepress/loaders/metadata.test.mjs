import assert from 'node:assert/strict'
import { test } from 'node:test'

import { field, wrappedField } from './metadata.mjs'

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

test('wrappedField folds a wrapped value onto one line', () => {
    assert.equal(
        wrappedField(RFD, 'Summary'),
        'Introduces per-tool options for static user-defined configuration passed to tools at runtime.',
    )
})

test('wrappedField reads a value that does not wrap', () => {
    assert.equal(wrappedField(RFD, 'Status'), 'Implemented')
})

test('wrappedField stops at the next field', () => {
    // `Date` sits directly above `Extended by`, which is not indented and so
    // is not a continuation of it.
    assert.equal(wrappedField(RFD, 'Date'), '2026-01-01')
})

test('wrappedField stops at the blank line closing the block', () => {
    const trailing = `# RFD 042: Tool Options

- **Status**: Implemented
- **Summary**: One sentence.

Body prose that is not part of the summary.
`

    assert.equal(wrappedField(trailing, 'Summary'), 'One sentence.')
})

test('wrappedField reports an absent field as null', () => {
    assert.equal(wrappedField(RFD, 'Supersedes'), null)
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
    const quoting = RFD + EXAMPLE

    assert.equal(field(quoting, 'Status'), 'Implemented')
    assert.equal(
        wrappedField(quoting, 'Summary'),
        'Introduces per-tool options for static user-defined configuration passed to tools at runtime.',
    )
})

test('a field absent from the header is null, example or no example', () => {
    // The case that matters: reading the example here would report a summary
    // the document does not have, and the build check that rejects a published
    // RFD without one would pass it.
    const bare = `# RFD 001: The JP RFD Process

- **Status**: Implemented
- **Category**: Process

## Summary

Prose.
${EXAMPLE}`

    assert.equal(wrappedField(bare, 'Summary'), null)
    assert.equal(field(bare, 'Status'), 'Implemented')
})

test('a document with no metadata block reads as null', () => {
    assert.equal(field('# RFD 042: Tool Options\n\n## Summary\n\nProse.\n', 'Status'), null)
})
