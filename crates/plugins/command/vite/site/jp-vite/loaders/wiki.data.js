import { readSettings } from '../settings.mjs'
import { wikiEntries } from '../wiki.mjs'

// The wiki's pages, for the wiki index and the cross-reference widgets.

// What to watch: the settings, which name the wiki directory, and the pages
// and the order file. Paths are relative to this file, which sits three levels
// below the VitePress source directory, in `.vitepress/jp-vite/loaders/`.
function watched() {
    const route = readSettings().wiki?.route
    if (!route) return ['../settings.json']

    return ['../settings.json', `../../../${route}/*.md`, `../../../${route}/.order.json`]
}

export default {
    watch: watched(),
    load() {
        return wikiEntries()
    },
}
