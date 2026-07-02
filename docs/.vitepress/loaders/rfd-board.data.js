import { assembleBoard } from './rfd-shared.mjs'

// The priority board spans the whole open backlog: published RFDs (Discussion /
// Accepted) and the drafts you want to prioritise finishing. It deliberately
// mixes the two id spaces, so it has its own loader rather than reusing the
// published-only `rfds.data.js`, which feeds the public RFD index.

// Transient divider token. It only ever lives in the rendered board, so the
// cutoff line is draggable; it is never written back to priority.json.
const CUTOFF = '--cutoff--'

export default {
    load() {
        const { entries, priority } = assembleBoard()

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
