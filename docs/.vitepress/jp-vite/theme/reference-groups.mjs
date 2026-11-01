// The documents a page references, or is referenced by, grouped by kind for the
// list at the bottom of the page.
//
// No Node imports, so the browser can use it too.

// The kinds in the order the list shows them, and what a group of each is
// called.
const KINDS = [
    { kind: 'wiki', name: 'Wiki pages' },
    { kind: 'rfd', name: 'RFDs' },
    { kind: 'ticket', name: 'Tickets' },
]

// More references than this and a list starts folded, so a long one doesn't
// push the page's own text out of view.
export const FOLD_ABOVE = 6

// `entries`, documents as `theme/documents.mjs` describes them, as
// `{ kind, name, entries }` groups in the order above, each sorted by label;
// kinds without an entry are left out.
export function groupReferences(entries) {
    return KINDS
        .map(({ kind, name }) => ({
            kind,
            name,
            entries: entries
                .filter(entry => entry.kind === kind)
                .sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0)),
        }))
        .filter(group => group.entries.length > 0)
}

// How a reference names its document beside the title: `RFD 042`, `T-0aaaaaa`,
// `W-0aaaaaa`.
export function referenceLabel(entry) {
    return entry.kind === 'rfd' ? `RFD ${entry.label}` : entry.label
}
