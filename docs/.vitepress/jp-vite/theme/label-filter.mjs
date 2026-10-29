// Filtering tickets by their labels, from a page's query string.
//
// Two parameters, each repeatable, and each term a label as `jp ticket list
// --label` takes it: `key=value` matches that label, a bare `key` any value
// under the key, and both match without case.
//
// - `label`: a ticket has to carry every term, as repeated `--label` flags
//   narrow a listing.
// - `any-label`: a ticket has to carry at least one term.
//
// A link from elsewhere on the site uses them to show the tickets that name
// something, such as an RFD, without the board knowing what that something is.

// The filter in `search`, a page's query string, or `null` when it names none.
export function parseLabelFilter(search) {
    const params = new URLSearchParams(search ?? '')
    const terms = name => params.getAll(name).map(term => term.trim()).filter(Boolean)

    const all = terms('label')
    const any = terms('any-label')
    if (all.length === 0 && any.length === 0) return null

    return { all, any }
}

// Whether a ticket carrying `labels` passes `filter`; every ticket passes no
// filter.
export function matchesLabelFilter(labels, filter) {
    if (!filter) return true

    const carries = term => labels.some(label => matchesTerm(label, term))
    if (!filter.all.every(carries)) return false
    if (filter.any.length > 0 && !filter.any.some(carries)) return false

    return true
}

// Whether `label`, as a ticket carries it, is what `term` asks for.
function matchesTerm(label, term) {
    const folded = label.toLowerCase()
    const wanted = term.toLowerCase()
    if (wanted.includes('=')) return folded === wanted

    return folded === wanted || folded.startsWith(`${wanted}=`)
}

// The filter in words, for the banner above a filtered board.
export function describeLabelFilter(filter) {
    const parts = []
    if (filter.all.length > 0) parts.push(filter.all.join(' and '))
    if (filter.any.length > 0) parts.push(filter.any.join(' or '))

    return `Tickets labelled ${parts.join(', and ')}`
}

// The query string a link uses to show the tickets carrying any of `labels`.
export function anyLabelQuery(labels) {
    const params = new URLSearchParams()
    for (const label of labels) params.append('any-label', label)

    return `?${params}`
}
