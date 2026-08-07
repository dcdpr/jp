// Shared mechanics for the two boards: the RFD priority list and the ticket
// kanban.
//
// Both are dev-time editors. Dragging exists only on the dev server, which is
// also the only place with an endpoint to save to, so the production bundle
// never pulls in SortableJS and both boards render read-only.

export const isDev = import.meta.env.DEV

/// Drag behaviour both boards want: hold briefly to drag on touch so a swipe
/// still scrolls, immediate on desktop, and interactive elements stay clickable.
export const DRAG_DEFAULTS = {
    animation: 150,
    delay: 150,
    delayOnTouchOnly: true,
    touchStartThreshold: 5,
    filter: 'a, input, label, button',
}

/// Make `el` sortable, loading SortableJS on demand.
///
/// Returns `null` when there's no element, so callers can bind refs that may
/// not have rendered yet.
export async function createSortable(el, options) {
    if (!el) return null

    const { default: Sortable } = await import('sortablejs')

    return Sortable.create(el, { ...DRAG_DEFAULTS, ...options })
}

/// Read board state back from the dev server.
///
/// The data loader caches its result for the server's lifetime, so a refresh
/// would otherwise show what the page was built with. Returns `null` when
/// there's nothing to read, leaving the caller on its build-time data.
export async function loadBoard(endpoint) {
    try {
        const res = await fetch(endpoint)
        if (res.ok) return await res.json()
    } catch { /* keep the build-time data */ }

    return null
}

/// Persist board state, returning an error message or `null` on success.
export async function saveBoard(endpoint, body) {
    try {
        const res = await fetch(endpoint, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(body),
        })
        if (!res.ok) return await res.text()
    } catch (err) {
        return String(err.message || err)
    }

    return null
}
