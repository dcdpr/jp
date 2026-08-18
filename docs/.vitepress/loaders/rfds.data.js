import { readFileSync, readdirSync } from 'node:fs'
import { resolve } from 'node:path'

import {
    buildEntries,
    buildGraph,
    checkRelationshipDuplicates,
    checkRequiresOnImplemented,
    checkStatusGate,
    checkSummaries,
    checkTerminalOnBoard,
    findCycles,
    findDuplicateIds,
    findStrayDraftRefs,
    loadPriority,
} from './rfd-shared.mjs'

const rfdDir = resolve(import.meta.dirname, '../../rfd')
const cachePath = resolve(import.meta.dirname, '../rfd-summaries.json')
const priorityPath = resolve(import.meta.dirname, '../../rfd/.priority.json')

function loadSummaries() {
    try {
        return JSON.parse(readFileSync(cachePath, 'utf-8'))
    } catch {
        return {}
    }
}

// RFDs that legitimately describe the `DNN` numbering convention and so are
// allowed to mention draft ids. See `findStrayDraftRefs`.
const dnnAllowlist = new Set([
    '001-jp-rfd-process.md',
])

export default {
    load() {
        const files = readdirSync(rfdDir)
            .filter(f => /^\d{3}-.+\.md$/.test(f) && !f.startsWith('000-'))
            .sort()

        const summaries = loadSummaries()
        const graph = buildGraph(rfdDir, files)

        // Only published RFDs reach a terminal status, so the published graph
        // carries every status the board check needs.
        const priority = loadPriority(priorityPath)

        // Every validation aborts the published build.
        const errors = [
            checkSummaries(rfdDir, files, summaries),
            findDuplicateIds(files),
            findStrayDraftRefs(rfdDir, files, dnnAllowlist),
            checkRelationshipDuplicates(graph),
            checkStatusGate(graph),
            checkRequiresOnImplemented(graph),
            findCycles(graph),
            checkTerminalOnBoard(graph, priority),
        ]
        for (const error of errors) {
            if (error) throw new Error(error)
        }

        return buildEntries(rfdDir, files, summaries, '/rfd')
    },
}
