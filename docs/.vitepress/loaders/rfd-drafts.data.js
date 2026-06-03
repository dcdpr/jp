import { readdirSync } from 'node:fs'
import { resolve } from 'node:path'

import {
    buildEntries,
    buildGraph,
    checkRelationshipDuplicates,
    checkStatusGate,
    findCycles,
    findDuplicateIds,
} from './rfd-shared.js'

const draftsDir = resolve(import.meta.dirname, '../../rfd/drafts')

export default {
    load() {
        const files = readdirSync(draftsDir)
            .filter(f => /^D\d{2}-.+\.md$/.test(f))
            .sort()

        // Duplicate ids abort the build, exactly like published RFDs — that's
        // the failure mode that prompted bringing drafts back under
        // validation.
        const dup = findDuplicateIds(files)
        if (dup) throw new Error(dup)

        // Every other draft check is advisory. Drafts are work in progress,
        // so a relationship or cycle problem warns but does not fail the docs
        // build. Summaries and the stray-draft-reference rule don't apply to
        // drafts at all.
        const graph = buildGraph(draftsDir, files)
        const warnings = [
            checkRelationshipDuplicates(graph),
            checkStatusGate(graph),
            findCycles(graph),
        ]
        for (const warning of warnings) {
            if (warning) console.warn(`[rfd-drafts] ${warning}`)
        }

        return buildEntries(draftsDir, files, {}, '/rfd/drafts')
    },
}
