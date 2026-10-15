import {
    buildEntries,
    buildGraph,
    checkRelationshipDuplicates,
    checkRequiresOnImplemented,
    checkStatusGate,
    checkSummaries,
    checkTerminalOnBoard,
    DRAFTS,
    findCycles,
    findDuplicateIds,
    findStrayDraftRefs,
    findUndefinedRefLinks,
    listRfds,
    loadPriority,
    PUBLISHED,
    rfdLocations,
} from './rfd-shared.mjs'

// The published RFDs, for the RFD index and the cross-reference widgets.

// RFD 001 is the process document, which describes the `DNN` numbering
// convention and so may mention draft ids. See `findStrayDraftRefs`.
const isProcessDocument = f => f.startsWith('001-')

export default {
    // The settings name the RFD directory, so a change to them is a change to
    // what this reads.
    watch: ['../settings.json'],
    load() {
        const rfds = rfdLocations()
        if (!rfds) return []

        const files = listRfds(rfds.dir, PUBLISHED)
        const graph = buildGraph(rfds.dir, files)

        // The board mixes both id spaces, so the terminal check needs statuses
        // from both. Drafts are otherwise none of this loader's business: they
        // contribute nothing but a status here.
        const draftFiles = listRfds(rfds.drafts, DRAFTS)
        const boardGraph = new Map([
            ...graph,
            ...buildGraph(rfds.drafts, draftFiles),
        ])

        const priority = loadPriority(rfds.priority)

        // Every validation aborts the published build.
        const errors = [
            checkSummaries(rfds.dir, files),
            findDuplicateIds(files),
            findStrayDraftRefs(rfds.dir, files, new Set(files.filter(isProcessDocument))),
            findUndefinedRefLinks(rfds.dir, files),
            checkRelationshipDuplicates(graph),
            checkStatusGate(graph),
            checkRequiresOnImplemented(graph),
            findCycles(graph),
            checkTerminalOnBoard(boardGraph, priority),
        ]
        for (const error of errors) {
            if (error) throw new Error(error)
        }

        return buildEntries(rfds.dir, files, rfds.path)
    },
}
