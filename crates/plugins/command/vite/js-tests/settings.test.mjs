import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

import { readSettings } from '../site/jp-vite/settings.mjs'

// Every page the site renders reads the snapshot, so a broken one fails them
// all; the error has to say which file to fix rather than leave each page to
// take the blame. A conflicted merge is how the snapshot most often breaks.
test('a snapshot that is not JSON is named in the error', () => {
    const dir = mkdtempSync(join(tmpdir(), 'jp-vite-settings-'))
    try {
        const file = join(dir, 'settings.json')
        writeFileSync(file, '{\n<<<<<<< HEAD\n  "site": "docs"\n}\n')

        // Only the start of the message: the parser's own wording differs
        // between Node versions.
        assert.throws(
            () => readSettings(file),
            error => error.message.startsWith(`${file} is not valid JSON: `),
        )
    } finally {
        rmSync(dir, { recursive: true, force: true })
    }
})
