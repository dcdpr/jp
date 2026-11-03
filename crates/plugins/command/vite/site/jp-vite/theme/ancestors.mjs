// The documents above a document: its parent, that one's parent, and so on.
//
// A wiki page and a ticket each name the document they sit under in a `Parent`
// field, read as `parent`.
//
// No Node imports, so the browser can use it too.

// The documents above `id`, the top one first. Empty for one under nothing.
//
// `find` maps an id to its document, or `null` for an id the walk should stop
// at: one that isn't a document, or one of a kind the trail doesn't cross
// into. A parent already passed stops it too, so parents that loop still give
// a finite trail.
export function ancestorsOf(id, find) {
    const seen = new Set([id])
    const trail = []

    let parent = find(id)?.parent
    while (parent && !seen.has(parent)) {
        const found = find(parent)
        if (!found) break

        seen.add(parent)
        trail.unshift(found)
        parent = found.parent
    }

    return trail
}
