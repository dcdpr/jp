import { readSettings } from '../settings.mjs'
import { assembleBoard } from './ticket-shared.mjs'

// Tickets for the ticket index and the board.
//
// Both pages read the same loader: the index lists every ticket regardless of
// status, and the board takes the columns, which are ordered by the board file,
// `.board.json`, and the number of Done tickets it shows when unfiltered.

// What to watch: the settings, which name the ticket directory, and the
// tickets themselves. Paths are relative to this file, which sits three levels
// below the VitePress source directory, in `.vitepress/jp-vite/loaders/`.
function watched() {
    const route = readSettings().ticket?.route
    if (!route) return ['../settings.json']

    return ['../settings.json', `../../../${route}/*.md`, `../../../${route}/.board.json`]
}

export default {
    watch: watched(),
    load() {
        const { tickets, columns, doneLimit } = assembleBoard()

        return { tickets, columns, doneLimit }
    },
}
