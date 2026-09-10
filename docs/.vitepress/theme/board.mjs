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

/// POST to a dev-server endpoint, returning whether it succeeded and what it
/// said.
///
/// `output` carries the endpoint's message on success as well as on failure.
/// It is empty when an endpoint answers a bare `{ ok: true }`, and holds the
/// response body when a request is rejected.
export async function postBoard(endpoint, body) {
    let res
    let text
    try {
        res = await fetch(endpoint, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(body),
        })
        text = (await res.text()).trim()
    } catch (err) {
        return { ok: false, output: String(err.message || err) }
    }

    // Success is JSON; a rejected request answers in plain text.
    let output = text
    try {
        const parsed = JSON.parse(text)
        output = typeof parsed?.output === 'string' ? parsed.output.trim() : ''
    } catch { /* keep the plain-text body */ }

    return { ok: res.ok, output: output || (res.ok ? '' : res.statusText) }
}

/// Persist board state, returning an error message or `null` on success.
export async function saveBoard(endpoint, body) {
    const { ok, output } = await postBoard(endpoint, body)

    return ok ? null : output
}
