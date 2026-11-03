// Tickets filed under a parent, for the ticket board and a ticket's page.
//
// A ticket names its parent in its `Parent` field, and a parent names nothing,
// so its children are the tickets naming it. The board narrows to one parent's
// group with `?under=T-0a1b2c3`: the parent and its children.
//
// No Node imports, so the browser can use it too.

// The parent `search`, a page's query string, narrows to, or `null`.
export function parseParentFilter(search) {
    const under = new URLSearchParams(search ?? '').get('under')?.trim()

    return under ? under : null
}

// `search` with its parent filter replaced by `parent`, and its other
// parameters kept; empty when nothing is left.
export function withParentFilter(search, parent) {
    const params = new URLSearchParams(search ?? '')
    params.delete('under')
    if (parent) params.set('under', parent)
    const query = params.toString()

    return query ? `?${query}` : ''
}

// Whether `ticket` belongs to the group under `parent`: the parent itself, or
// one of its children. Every ticket belongs to no filter.
export function inGroup(ticket, parent) {
    return !parent || ticket.id === parent || ticket.parent === parent
}

// The tickets among `tickets` filed under `parent`, in the order `tickets`
// holds them.
export function childrenOf(tickets, parent) {
    return tickets.filter(ticket => ticket.parent === parent && ticket.id !== parent)
}

// How many of `children` are Done, and how many there are.
export function progress(children) {
    return {
        done: children.filter(child => child.status === 'Done').length,
        total: children.length,
    }
}
