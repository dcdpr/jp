import {
    buildEntries,
    buildGraph,
    checkRelationshipDuplicates,
    checkRequiresOnImplemented,
    checkStatusGate,
    DRAFTS,
    findCycles,
    findDuplicateIds,
    findUndefinedRefLinks,
    listRfds,
    PUBLISHED,
    rfdLocations,
} from './rfd-shared.mjs'

// The draft RFDs, for the drafts index and the cross-reference widgets.

export default {
    // The settings name the RFD directory, so a change to them is a change to
    // what this reads.
    watch: ['../settings.json'],
    load() {
        const rfds = rfdLocations()
        if (!rfds) return []

        const files = listRfds(rfds.drafts, DRAFTS)

        // Duplicate ids abort the build, exactly like published RFDs — that's
        // the failure mode that prompted bringing drafts back under
        // validation.
        const dup = findDuplicateIds(files)
        if (dup) throw new Error(dup)

        // Every other draft check is advisory. Drafts are work in progress,
        // so a relationship or cycle problem warns but does not fail the docs
        // build. Summaries and the stray-draft-reference rule don't apply to
        // drafts at all.
        const graph = buildGraph(rfds.drafts, files)

        // `checkRequiresOnImplemented` needs the *published* statuses to tell
        // whether a draft's `Requires` target is already Implemented (a draft
        // can require a published RFD). Merge a published graph in: draft
        // entries supply the `Requires` sources, published entries supply the
        // target statuses.
        const combined = new Map([
            ...buildGraph(rfds.dir, listRfds(rfds.dir, PUBLISHED)),
            ...graph,
        ])

        const warnings = [
            findUndefinedRefLinks(rfds.drafts, files),
            checkRelationshipDuplicates(graph),
            checkStatusGate(graph),
            checkRequiresOnImplemented(combined),
            findCycles(graph),
        ]
        for (const warning of warnings) {
            if (warning) console.warn(`[rfd-drafts] ${warning}`)
        }

        return buildEntries(rfds.drafts, files, rfds.draftsPath)
    },
}
