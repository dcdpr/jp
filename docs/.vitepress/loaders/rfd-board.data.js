import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

import {
    buildEntries,
    buildGraph,
    loadPriority,
    mergePriority,
    mergeDependencies,
    checkPriority,
} from './rfd-shared.js'

// The priority board spans the whole open backlog: published RFDs (Discussion /
// Accepted) and the drafts you want to prioritise finishing. It deliberately
// mixes the two id spaces, so it has its own loader rather than reusing the
// published-only `rfds.data.js`, which feeds the public RFD index.

const rfdDir = resolve(import.meta.dirname, '../../rfd')
const draftsDir = resolve(import.meta.dirname, '../../rfd/drafts')
const cachePath = resolve(import.meta.dirname, '../rfd-summaries.json')
const priorityPath = resolve(import.meta.dirname, '../../rfd/priority.json')

// Transient divider token. It only ever lives in the rendered board, so the
// cutoff line is draggable; it is never written back to priority.json.
const CUTOFF = '--cutoff--'

function loadSummaries() {
    try {
        return JSON.parse(readFileSync(cachePath, 'utf-8'))
    } catch {
        return {}
    }
}

export default {
    load() {
        const publishedFiles = readdirSync(rfdDir)
            .filter(f => /^\d{3}-.+\.md$/.test(f) && !f.startsWith('000-'))
            .sort()
        const draftFiles = readdirSync(draftsDir)
            .filter(f => /^D\d{2}-.+\.md$/.test(f))
            .sort()

        const summaries = loadSummaries()
        const entries = [
            ...buildEntries(rfdDir, publishedFiles, summaries, '/rfd'),
            ...buildEntries(draftsDir, draftFiles, {}, '/rfd/drafts'),
        ]

        // Combined dependency graph so the ordering constraint spans both id
        // spaces (a draft may require a published RFD).
        const graph = new Map([
            ...buildGraph(rfdDir, publishedFiles),
            ...buildGraph(draftsDir, draftFiles),
        ])

        const priority = loadPriority(priorityPath)
        mergePriority(entries, priority)
        mergeDependencies(entries, graph)
        const error = checkPriority(entries, priority)
        if (error) throw new Error(error)

        // UI-only divider between the prioritised list (`order`) and the
        // backlog. The board splits the combined list back into `order` /
        // `backlog` on save, so this is never persisted. The fractional rank
        // drops it between the last `order` entry and the first `backlog` one.
        entries.push({
            num: CUTOFF,
            slug: CUTOFF,
            divider: true,
            status: 'Cutoff',
            title: '',
            path: '',
            summary: null,
            dependsOn: [],
            inDevelopment: false,
            priority: priority.order.length - 0.5,
        })

        return entries
    },
}
