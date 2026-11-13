import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { test } from 'node:test'

import { readSettings } from '../site/jp-vite/settings.mjs'

test('a snapshot that is not JSON is named in the error', () => {
    const dir = mkdtempSync(join(tmpdir(), 'jp-vite-settings-'))
    try {
        const file = join(dir, 'settings.json')
        writeFileSync(file, '{\n<<<<<<< HEAD\n  "site": "docs"\n}\n')

        assert.throws(
            () => readSettings(file),
            error => error.message.startsWith(`${file} is not valid JSON: `),
        )
    } finally {
        rmSync(dir, { recursive: true, force: true })
    }
})
