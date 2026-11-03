// Labels as a reader sees them: grouped by key, so `domain=llm` and
// `domain=mcp` read as `domain: llm, mcp`, each part leading to the documents
// that carry it.
//
// The pure half of `DocLabels.vue`, which renders labels wherever the site
// shows them. No imports, so the browser and the build can both use it.

// `labels`, as written, grouped by key in the order each key first appears:
// `[key, values]`, `values` empty for a bare label.
export function groupLabels(labels) {
    const groups = new Map()
    for (const label of labels) {
        const split = label.indexOf('=')
        const key = split < 0 ? label : label.slice(0, split)
        if (!groups.has(key)) groups.set(key, [])
        if (split >= 0) groups.get(key).push(label.slice(split + 1))
    }

    return [...groups]
}

// Where a label of a document of `kind` leads, given the site's `settings`:
// the listing of that kind, narrowed to the documents carrying `term`, a
// `key=value` or a bare `key` for any value under it. `null` for a kind with no
// such listing.
export function labelHref(settings, kind, term) {
    const listing = { ticket: 'board', wiki: 'all' }[kind]
    const route = settings?.[kind]?.route
    if (!listing || !route) return null

    return `/${route}/${listing}?label=${encodeURIComponent(term)}`
}

// `labels` as one attribute value: each label percent-encoded, separated by
// spaces.
//
// A page's header is markdown the site compiles into a Vue template, so its
// labels reach `<DocLabels />` as a static attribute. Encoded, a label can't
// close the attribute, open a tag, or start a `{{ }}` interpolation, whatever
// it holds.
export function encodeLabels(labels) {
    return labels.map(encodeURIComponent).join(' ')
}

// The labels `encodeLabels` wrote.
export function decodeLabels(text) {
    return text.split(/\s+/).filter(Boolean).map(decodeURIComponent)
}
