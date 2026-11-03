import { assembleBoard } from './rfd-shared.mjs'

// The priority board spans the whole open backlog: published RFDs (Discussion /
// Accepted) and the drafts you want to prioritise finishing. It deliberately
// mixes the two id spaces, so it has its own loader rather than reusing the
// published-only `rfds.data.js`, which feeds the public RFD index.
//
// Alongside the entries, the raw board layout (milestone groups, backlog) is
// passed through in priority.json's shape: the board component builds its marker
// rows — milestone lines and the unsorted cutoff — from it, since markers are
// also created, renamed, and removed at runtime.
//
// Each entry's `inDevelopment` flag and ticket count come from the tickets
// naming the RFD rather than from the board file (see `mergeTickets`). They
// change on the ticket board, and the board component re-reads them through
// the dev server's `/__rfd-tickets` endpoint.

export default {
    // The settings name the RFD and ticket directories, so a change to them is
    // a change to what this reads.
    watch: ['../settings.json'],
    load() {
        const { entries, priority } = assembleBoard()

        return {
            entries,
            priority: {
                planned: priority.planned,
                backlog: priority.backlog,
            },
        }
    },
}
