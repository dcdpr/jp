import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

import { buildGraph } from './rfd-shared.mjs'

// Build the relationship graph from one RFD written to a scratch directory.
function graphOf(filename, content) {
    const dir = mkdtempSync(join(tmpdir(), 'rfd-graph-'))
    try {
        writeFileSync(join(dir, filename), content)
        return buildGraph(dir, [filename])
    } finally {
        rmSync(dir, { recursive: true, force: true })
    }
}

test('a relationship field wrapped onto a second line keeps every id', () => {
    // `comfort` wraps a metadata value like any other paragraph once it
    // outgrows the line. An id on the continuation line is as much a
    // relationship as one on the first.
    const graph = graphOf('072-command-plugin-system.md', `# RFD 072: Command Plugin System

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-06
- **Extended by**: [RFD 077], [RFD 109], [RFD 113], [RFD 114], [RFD 116],
  [RFD 121]
- **Summary**: Standalone command plugins communicate with JP via JSON-lines
  protocol to extend subcommands across languages.

## Summary

Prose.
`)

    assert.deepEqual(graph.get('072').extendedBy, ['077', '109', '113', '114', '116', '121'])
})

test('an id split from its prefix by the wrap still counts', () => {
    const graph = graphOf('122-release-pipeline.md', `# RFD 122: Release Pipeline

- **Status**: Discussion
- **Requires**: [RFD 116], [RFD 117], [RFD 118], [RFD 119], [RFD 120], [RFD
  121]
- **Summary**: One sentence.

## Summary

Prose.
`)

    assert.deepEqual(graph.get('122').requires, ['116', '117', '118', '119', '120', '121'])
})

test('a relationship quoted in the body is not read as the header\'s', () => {
    // RFD 001 documents the metadata format in a fenced block. A header
    // without `Requires` must not pick up the example's.
    const graph = graphOf('001-jp-rfd-process.md', `# RFD 001: The JP RFD Process

- **Status**: Implemented
- **Summary**: One sentence.

## Summary

\`\`\`markdown
- **Requires**: [RFD 042]
\`\`\`
`)

    assert.deepEqual(graph.get('001').requires, [])
})
