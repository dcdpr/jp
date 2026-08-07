import { assembleBoard } from './ticket-shared.mjs'

// Tickets for the `/ticket/` index and the board at `/ticket/board`.
//
// Both pages read the same loader: the index lists every ticket regardless of
// status, and the board takes the columns, which are ordered by
// `docs/ticket/board.json` and capped in Done.

export default {
    watch: ['../../ticket/*.md', '../../ticket/board.json'],
    load() {
        const { tickets, columns } = assembleBoard()

        return { tickets, columns }
    },
}
