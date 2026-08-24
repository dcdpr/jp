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
// Each entry's `inDevelopment` flag comes from ticket state rather than from the
// board file, so the board reports it without being able to set it.

export default {
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
