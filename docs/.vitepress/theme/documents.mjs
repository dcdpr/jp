import { data as drafts } from '../loaders/rfd-drafts.data.js'
import { data as published } from '../loaders/rfds.data.js'
import { data as tickets } from '../loaders/tickets.data.js'

// The site's cross-linkable documents — RFDs and tickets — in one shape.
//
// RFDs and tickets are different kinds of document with different lifecycles,
// but they link to each other constantly, and the widgets that follow those
// links (breadcrumbs, link colouring, references) shouldn't care which kind
// they're pointing at. This is the seam where the two become one list.

/// What each kind is called and where a trail starting cold points back to.
export const KINDS = {
    rfd: { name: 'RFDs', root: '/rfd/' },
    ticket: { name: 'Tickets', root: '/ticket/' },
}

/// The pages a trail can start from.
///
/// Landing on one resets the trail and becomes the crumb it roots at, so
/// following a ticket off the board leads back to the board rather than to the
/// list you didn't come from.
export const ORIGINS = [
    { path: '/rfd/', name: 'RFDs' },
    { path: '/rfd/drafts/', name: 'Drafts' },
    { path: '/rfd/priority', name: 'Priorities' },
    { path: '/ticket/', name: 'Tickets' },
    { path: '/ticket/board', name: 'Board' },
]

/// Every document, newest kind-agnostic shape.
///
/// `label` is what a breadcrumb or badge shows (`042`, `T0007`); `status` feeds
/// the link colouring.
export const DOCUMENTS = [
    ...published.map(entry => document('rfd', entry.num, entry)),
    ...drafts.map(entry => document('rfd', entry.num, entry)),
    ...tickets.tickets.map(entry => document('ticket', entry.id, entry)),
]

function document(kind, label, entry) {
    return {
        kind,
        label,
        title: entry.title,
        status: entry.status,
        path: entry.path,
        links: entry.links ?? [],
    }
}

const BY_PATH = new Map(DOCUMENTS.map(entry => [entry.path, entry]))
const BY_LABEL = new Map(DOCUMENTS.map(entry => [entry.label, entry]))

// Cross-kind reference edges, resolved once. A ticket citing an RFD and an RFD
// citing a ticket are the same relationship read from opposite ends, so both
// directions come from one pass over the union.
for (const entry of DOCUMENTS) {
    entry.references = (entry.links ?? [])
        .map(label => BY_LABEL.get(label))
        .filter(target => target && target !== entry)
    entry.referencedBy = []
}
for (const entry of DOCUMENTS) {
    for (const target of entry.references) target.referencedBy.push(entry)
}

/// Normalise a route or link path to the form [`DOCUMENTS`] uses.
function normalize(path) {
    return (path ?? '').replace(/\.html$/, '').replace(/(.)\/$/, '$1')
}

/// The document a site path points at, if any.
///
/// Matching is by path rather than by pattern, so a link only resolves when the
/// document it names actually exists.
export function documentAt(path) {
    return BY_PATH.get(normalize(path)) ?? null
}

/// The origin a path is, if it is one.
export function originAt(path) {
    const clean = normalize(path)

    return ORIGINS.find(origin => normalize(origin.path) === clean) ?? null
}

/// The tooltip shown on a link to `entry`.
export function describe(entry) {
    const prefix = entry.kind === 'rfd' ? `RFD ${entry.label}` : entry.label

    return entry.title ? `${prefix}: ${entry.title}` : prefix
}
