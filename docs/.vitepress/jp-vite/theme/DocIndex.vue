<script setup>
import { ref, computed, watch, onMounted, useSlots } from 'vue'
import { inBrowser } from 'vitepress'

// One index for every kind of document the site lists. RFDs and tickets differ
// in what their id column is called and what they filter by; everything else —
// the toolbar, the URL sync, the sortable table, the badges — is shared.
const props = defineProps({
    entries: { type: Array, required: true },
    // The status column and the `status:` search filter only make sense when
    // entries carry meaningful statuses. The drafts index hides them.
    showStatus: { type: Boolean, default: true },
    // Prefix for the persisted toolbar state, so two indexes on the same site
    // don't share filter/search/summary toggles.
    storageKey: { type: String, default: 'rfd' },
    // Heading of the id column, and the entry field holding that id.
    idLabel: { type: String, default: 'RFD' },
    idField: { type: String, default: 'num' },
    // The toolbar's filter buttons, and the entry field they match against.
    // `all` is prepended.
    filters: { type: Array, default: () => ['design', 'decision', 'guide', 'process'] },
    filterField: { type: String, default: 'category' },
    // Whether entries carry a one-line summary worth toggling.
    showSummary: { type: Boolean, default: true },
    // Tickets list their author; RFDs don't, since theirs is nearly always the
    // same person.
    showAuthors: { type: Boolean, default: false },
})

// A filled `actions` slot adds a trailing column, one cell per row, receiving
// that row's entry. Nothing here knows what the buttons do — the ticket index
// puts edit and delete there, the RFD index leaves it empty and gets no column.
const slots = useSlots()
const hasActions = computed(() => Boolean(slots.actions))

function stored(key, fallback) {
    try { return sessionStorage.getItem(key) ?? fallback } catch { return fallback }
}

const k = (name) => `${props.storageKey}-${name}`

// URL query params take precedence over sessionStorage, so a shared link like
// /rfd/?search=plugin shows the same filtered list for everyone.
function fromUrl(name) {
    if (!inBrowser) return null
    return new URLSearchParams(window.location.search).get(name)
}

// Default to descending by id so the newest RFDs sit at the top.
const sortKey = ref(props.idField)
const sortAsc = ref(false)
const categories = computed(() => ['all', ...props.filters])

const urlFilter = fromUrl('filter')
const filter = ref(categories.value.includes(urlFilter) ? urlFilter : stored(k('filter'), 'all'))
const search = ref(fromUrl('search') ?? stored(k('search'), ''))
const showSummaries = ref(stored(k('summaries'), 'true') === 'true')

// Mirror the toolbar state into the URL so the current view is shareable.
// replaceState (not pushState) keeps typing from flooding the history stack.
function syncUrl() {
    if (!inBrowser) return
    const params = new URLSearchParams(window.location.search)
    if (filter.value !== 'all') params.set('filter', filter.value)
    else params.delete('filter')
    if (search.value.trim()) params.set('search', search.value)
    else params.delete('search')
    const qs = params.toString()
    const url = window.location.pathname + (qs ? `?${qs}` : '') + window.location.hash
    window.history.replaceState(window.history.state, '', url)
}

watch(filter, v => { try { sessionStorage.setItem(k('filter'), v) } catch {}; syncUrl() })
watch(search, v => { try { sessionStorage.setItem(k('search'), v) } catch {}; syncUrl() })
watch(showSummaries, v => { try { sessionStorage.setItem(k('summaries'), String(v)) } catch {} })

// State restored from sessionStorage isn't in the URL yet; sync once so the
// address bar always matches what's on screen.
onMounted(syncUrl)

const showCategory = computed(() => filter.value === 'all')

// `optional` columns are the ones a narrow screen drops. Header cells and body
// cells both take the class from here, so the two can't disagree about which
// columns exist — keying the CSS off the field name meant `kind` never matched
// the rule written for `category`.
const columns = computed(() => {
    const cols = [
        { key: props.idField, label: props.idLabel },
        { key: 'title', label: 'Title' },
    ]
    if (showCategory.value) {
        cols.push({
            key: props.filterField,
            label: capitalize(props.filterField),
            optional: true,
        })
    }
    if (props.showAuthors) {
        cols.push({ key: 'authors', label: 'Authors', optional: true })
    }
    if (props.showStatus) cols.push({ key: 'status', label: 'Status' })
    return cols
})

function capitalize(value) {
    return value.charAt(0).toUpperCase() + value.slice(1)
}

// `In Progress` and the like need a class-safe form.
function statusClass(status) {
    return `doc-badge--${(status ?? 'unknown').toLowerCase().replace(/\s+/g, '-')}`
}

function toggleSort(key) {
    if (sortKey.value === key) {
        sortAsc.value = !sortAsc.value
    } else {
        sortKey.value = key
        sortAsc.value = true
    }
}

// Parse structured filters (e.g. `status:draft`) out of the search string.
const parsedSearch = computed(() => {
    const raw = search.value.trim()
    const statusMatch = props.showStatus ? raw.match(/\bstatus:(\S+)/i) : null
    const statusFilter = statusMatch ? statusMatch[1].toLowerCase() : null
    const textQuery = raw.replace(/\bstatus:\S+/gi, '').trim().toLowerCase()
    return { statusFilter, textQuery }
})

function toggleStatusFilter(status) {
    const s = status?.toLowerCase()
    if (!s) return
    const { statusFilter } = parsedSearch.value
    if (statusFilter === s) {
        search.value = search.value.replace(/\bstatus:\S+/gi, '').trim()
    } else if (/\bstatus:\S+/i.test(search.value)) {
        search.value = search.value.replace(/\bstatus:\S+/gi, `status:${s}`).trim()
    } else {
        search.value = (search.value.trim() + ` status:${s}`).trim()
    }
}

const filtered = computed(() => {
    let rows = filter.value === 'all'
        ? [...props.entries]
        : props.entries.filter(r => r[props.filterField]?.toLowerCase() === filter.value)

    const { statusFilter, textQuery } = parsedSearch.value

    if (statusFilter) {
        rows = rows.filter(r => r.status?.toLowerCase() === statusFilter)
    }

    if (textQuery) {
        rows = rows.filter(r =>
            [r[props.idField], r.title, r[props.filterField], r.status, r.summary, ...(r.labels ?? [])]
                .some(v => v?.toLowerCase().includes(textQuery))
        )
    }

    rows.sort((a, b) => {
        const av = (a[sortKey.value] ?? '').toLowerCase()
        const bv = (b[sortKey.value] ?? '').toLowerCase()
        if (av < bv) return sortAsc.value ? -1 : 1
        if (av > bv) return sortAsc.value ? 1 : -1
        return 0
    })

    return rows
})
</script>

<template>
<div class="doc-toolbar">
    <div class="doc-filters">
        <button
            v-for="cat in categories"
            :key="cat"
            :class="['doc-filter', { active: filter === cat }]"
            @click="filter = cat"
        >{{ cat }}</button>
    </div>
    <div class="doc-search-wrap">
        <input
            v-model="search"
            class="doc-search"
            type="text"
            :placeholder="showStatus ? 'Filter… e.g. status:' + (filters[0] ?? '') : 'Filter…'"
        />
        <button v-if="search" class="doc-search-clear" @click="search = ''" title="Clear">&times;</button>
    </div>
    <button
        v-if="showSummary"
        :class="['doc-toggle', { active: showSummaries }]"
        :title="showSummaries ? 'Hide summaries' : 'Show summaries'"
        @click="showSummaries = !showSummaries"
    >{{ showSummaries ? '⊟' : '⊞' }}</button>
</div>

<table class="doc-table">
<colgroup>
    <col style="width: 4rem">
    <col>
    <col v-if="showCategory" class="doc-col-optional" style="width: 7rem">
    <col v-if="showAuthors" class="doc-col-optional" style="width: 10rem">
    <col v-if="showStatus" style="width: 8rem">
    <col v-if="hasActions" style="width: 5.5rem">
</colgroup>
<thead><tr>
    <th v-for="col in columns" :key="col.key" :class="['doc-sortable', 'doc-col-' + col.key, { 'doc-col-optional': col.optional }]" @click="toggleSort(col.key)">
        {{ col.label }} <span class="doc-sort-arrow">{{ sortKey === col.key ? (sortAsc ? '▲' : '▼') : '' }}</span>
    </th>
    <th v-if="hasActions" class="doc-col-actions"></th>
</tr></thead>
<tbody>
<tr v-for="entry in filtered" :key="entry.slug">
    <td>{{ entry[idField] }}</td>
    <td>
        <a :href="entry.path">{{ entry.title }}</a>
        <span v-if="entry.blockedBy" class="doc-badge doc-badge--blocked">blocked by {{ entry.blockedBy }}</span>
        <span v-for="label in entry.labels ?? []" :key="label" class="doc-badge doc-badge--label">{{ label }}</span>
        <div v-if="showSummaries && entry.summary" class="doc-summary">{{ entry.summary }}</div>
    </td>
    <td v-if="showCategory" class="doc-col-optional">{{ entry[filterField] }}</td>
    <td v-if="showAuthors" class="doc-col-optional">{{ (entry.authors ?? '').replace(/\s*<[^>]*>/, '') }}</td>
    <td v-if="showStatus"><span
        :class="['doc-badge', statusClass(entry.status), { 'doc-badge--active': parsedSearch.statusFilter === entry.status?.toLowerCase() }]"
        @click="toggleStatusFilter(entry.status)"
    >{{ entry.status }}</span></td>
    <td v-if="hasActions" class="doc-col-actions">
        <slot name="actions" :entry="entry" />
    </td>
</tr>
</tbody>
</table>
</template>

<style>
.doc-toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 2rem;
}
.doc-toolbar .doc-toggle {
    margin-left: auto;
}
.doc-filters {
    display: flex;
    gap: 0.5rem;
}
.doc-search-wrap {
    position: relative;
    width: 14rem;
}
.doc-search {
    padding: 0.3rem 1.75rem 0.3rem 0.75rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 4px;
    background: transparent;
    color: var(--vp-c-text-1);
    font-size: 0.9rem;
    outline: none;
    width: 100%;
    box-sizing: border-box;
}
.doc-search::placeholder {
    color: var(--vp-c-text-3);
}
.doc-search:focus {
    border-color: var(--vp-c-brand-1);
}
.doc-search-clear {
    position: absolute;
    right: 0.35rem;
    top: 50%;
    transform: translateY(-50%);
    width: 1.2rem;
    height: 1.2rem;
    border-radius: 50%;
    border: none;
    background: var(--vp-c-divider);
    color: var(--vp-c-text-2);
    cursor: pointer;
    font-size: 0.85rem;
    line-height: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
}
.doc-search-clear:hover {
    background: var(--vp-c-text-3);
    color: var(--vp-c-bg);
}
.doc-filter {
    padding: 0.25rem 0.75rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 4px;
    background: var(--vp-c-bg-soft);
    color: var(--vp-c-text-2);
    cursor: pointer;
    font-size: 0.9rem;
    text-transform: capitalize;
}
.doc-toggle {
    padding: 0;
    border: none;
    background: transparent;
    color: var(--vp-c-text-3);
    cursor: pointer;
    font-size: 1.1rem;
    line-height: 1;
}
.doc-filter:hover {
    border-color: var(--vp-c-brand-1);
    color: var(--vp-c-text-1);
}
.doc-filter.active {
    border-color: var(--vp-c-brand-1);
    background: var(--vp-c-brand-1);
    color: var(--vp-c-white);
}
.doc-sortable {
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
}
.doc-sortable:hover {
    color: var(--vp-c-brand-1);
}
.doc-sort-arrow {
    font-size: 0.7em;
    margin-left: 0.2em;
}
.doc-table {
    margin-top: 0.5em !important;
    table-layout: fixed !important;
    width: 100% !important;
    max-width: 100% !important;
    display: table !important;
}
.doc-summary {
    font-size: 0.8rem;
    color: var(--vp-c-text-2);
    line-height: 1.4;
    margin-top: 0.15rem;
}
.doc-table .doc-badge {
    cursor: pointer;
    transition: opacity 0.15s, box-shadow 0.15s;
}
.doc-table .doc-badge:hover {
    opacity: 0.8;
}
.doc-badge--active {
    box-shadow: 0 0 0 2px var(--vp-c-brand-1);
}
@media (max-width: 767px) {
    .doc-table {
        table-layout: auto !important;
    }
}
@media (max-width: 639px) {
    .doc-toolbar {
        flex-wrap: wrap;
    }
    .doc-filters {
        width: 100%;
        overflow-x: auto;
        -webkit-overflow-scrolling: touch;
    }
    .doc-filter {
        font-size: 0.8rem;
        padding: 0.2rem 0.5rem;
        white-space: nowrap;
    }
    .doc-search-wrap {
        flex: 1;
        min-width: 0;
        width: auto;
    }
    .doc-search {
        font-size: 1rem;
    }
    .doc-col-optional {
        display: none;
    }
    .doc-col-actions {
        white-space: nowrap;
    }
    .doc-badge {
        font-size: 0.75rem;
        padding: 0.1rem 0.4rem;
    }
}
</style>
