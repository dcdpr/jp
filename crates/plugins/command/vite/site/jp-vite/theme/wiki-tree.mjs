// The wiki's pages as a tree: which pages sit under a page, and which are at
// the top.
//
// Reads the entries the wiki loader gives, each carrying its `parent`, and its
// `depth` and `position` in the reading order (`null` for an archived page).
// No Node imports, so the browser can use it too.

// The active entries `keep` holds, in reading order.
function inOrder(entries, keep) {
    return entries
        .filter(entry => entry.position !== null && entry.position !== undefined && keep(entry))
        .sort((a, b) => a.position - b.position)
}

// The active pages directly under the page `id`, in reading order.
export function childrenOf(entries, id) {
    const parent = entries.find(entry => entry.id === id)
    if (!parent || parent.depth === null || parent.depth === undefined) return []

    return inOrder(entries, entry => entry.parent === id && entry.depth === parent.depth + 1)
}

// The active pages without a parent, in reading order.
export function topPages(entries) {
    return inOrder(entries, entry => entry.depth === 0)
}
