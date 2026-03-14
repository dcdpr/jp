---
aside: false
prev: false
next: false
---

# Requests for Discussion

RFDs are short design documents that describe a significant change before
implementation begins. See [RFD 001](./001-jp-rfd-process) for the full process.

- **Design** — feature proposals and architectural changes
- **Decision** — recording a specific choice: a technology, convention, or standard
- **Guide** — how-tos and reference material for contributors
- **Process** — how the project operates: workflows, policies, values

<script setup>
import { ref, computed, watch } from 'vue'
import { data } from '../.vitepress/loaders/rfds.data.js'

function stored(key, fallback) {
    try { return sessionStorage.getItem(key) ?? fallback } catch { return fallback }
}

const filter = ref(stored('rfd-filter', 'all'))
const search = ref(stored('rfd-search', ''))
const showSummaries = ref(stored('rfd-summaries', 'true') === 'true')

watch(filter, v => { try { sessionStorage.setItem('rfd-filter', v) } catch {} })
watch(search, v => { try { sessionStorage.setItem('rfd-search', v) } catch {} })
watch(showSummaries, v => { try { sessionStorage.setItem('rfd-summaries', String(v)) } catch {} })
const sortKey = ref('num')
const sortAsc = ref(true)
const categories = ['all', 'design', 'decision', 'guide', 'process']

const showCategory = computed(() => filter.value === 'all')

const columns = computed(() => {
    const cols = [
        { key: 'num', label: 'RFD' },
        { key: 'title', label: 'Title' },
    ]
    if (showCategory.value) cols.push({ key: 'category', label: 'Category' })
    cols.push({ key: 'status', label: 'Status' })
    return cols
})

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
    const statusMatch = raw.match(/\bstatus:(\S+)/i)
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
        ? [...data]
        : data.filter(r => r.category?.toLowerCase() === filter.value)

    const { statusFilter, textQuery } = parsedSearch.value

    if (statusFilter) {
        rows = rows.filter(r => r.status?.toLowerCase() === statusFilter)
    }

    if (textQuery) {
        rows = rows.filter(r =>
            [r.title, r.category, r.status, r.summary]
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

<div class="rfd-toolbar">
    <div class="rfd-filters">
        <button
            v-for="cat in categories"
            :key="cat"
            :class="['rfd-filter', { active: filter === cat }]"
            @click="filter = cat"
        >{{ cat }}</button>
    </div>
    <div class="rfd-search-wrap">
        <input
            v-model="search"
            class="rfd-search"
            type="text"
            placeholder="Filter… e.g. status:draft"
        />
        <button v-if="search" class="rfd-search-clear" @click="search = ''" title="Clear">&times;</button>
    </div>
    <button
        :class="['rfd-toggle', { active: showSummaries }]"
        :title="showSummaries ? 'Hide summaries' : 'Show summaries'"
        @click="showSummaries = !showSummaries"
    >{{ showSummaries ? '⊟' : '⊞' }}</button>
</div>

<table class="rfd-table">
<colgroup>
    <col style="width: 4rem">
    <col>
    <col v-if="showCategory" class="rfd-col-category" style="width: 7rem">
    <col style="width: 8rem">
</colgroup>
<thead><tr>
    <th v-for="col in columns" :key="col.key" :class="['rfd-sortable', 'rfd-col-' + col.key]" @click="toggleSort(col.key)">
        {{ col.label }} <span class="rfd-sort-arrow">{{ sortKey === col.key ? (sortAsc ? '▲' : '▼') : '' }}</span>
    </th>
</tr></thead>
<tbody>
<tr v-for="rfd in filtered" :key="rfd.slug">
    <td>{{ rfd.num }}</td>
    <td>
        <a :href="'./' + rfd.slug">{{ rfd.title }}</a>
        <div v-if="showSummaries && rfd.summary" class="rfd-summary">{{ rfd.summary }}</div>
    </td>
    <td v-if="showCategory" class="rfd-col-category">{{ rfd.category }}</td>
    <td><span
        :class="['rfd-badge', 'rfd-badge--' + (rfd.status?.toLowerCase() ?? 'unknown'), { 'rfd-badge--active': parsedSearch.statusFilter === rfd.status?.toLowerCase() }]"
        @click="toggleStatusFilter(rfd.status)"
    >{{ rfd.status }}</span></td>
</tr>
</tbody>
</table>

<style>
.rfd-toolbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 2rem;
}
.rfd-toolbar .rfd-toggle {
    margin-left: auto;
}
.rfd-filters {
    display: flex;
    gap: 0.5rem;
}
.rfd-search-wrap {
    position: relative;
    width: 14rem;
}
.rfd-search {
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
.rfd-search::placeholder {
    color: var(--vp-c-text-3);
}
.rfd-search:focus {
    border-color: var(--vp-c-brand-1);
}
.rfd-search-clear {
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
.rfd-search-clear:hover {
    background: var(--vp-c-text-3);
    color: var(--vp-c-bg);
}
.rfd-filter {
    padding: 0.25rem 0.75rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 4px;
    background: var(--vp-c-bg-soft);
    color: var(--vp-c-text-2);
    cursor: pointer;
    font-size: 0.9rem;
    text-transform: capitalize;
}
.rfd-toggle {
    padding: 0;
    border: none;
    background: transparent;
    color: var(--vp-c-text-3);
    cursor: pointer;
    font-size: 1.1rem;
    line-height: 1;
}
.rfd-filter:hover {
    border-color: var(--vp-c-brand-1);
    color: var(--vp-c-text-1);
}
.rfd-filter.active {
    border-color: var(--vp-c-brand-1);
    background: var(--vp-c-brand-1);
    color: var(--vp-c-white);
}
.rfd-sortable {
    cursor: pointer;
    user-select: none;
    white-space: nowrap;
}
.rfd-sortable:hover {
    color: var(--vp-c-brand-1);
}
.rfd-sort-arrow {
    font-size: 0.7em;
    margin-left: 0.2em;
}
.rfd-table {
    margin-top: 0.5em !important;
    table-layout: fixed !important;
    width: 100% !important;
    max-width: 100% !important;
    display: table !important;
}
.rfd-summary {
    font-size: 0.8rem;
    color: var(--vp-c-text-2);
    line-height: 1.4;
    margin-top: 0.15rem;
}
.rfd-table .rfd-badge {
    cursor: pointer;
    transition: opacity 0.15s, box-shadow 0.15s;
}
.rfd-table .rfd-badge:hover {
    opacity: 0.8;
}
.rfd-badge--active {
    box-shadow: 0 0 0 2px var(--vp-c-brand-1);
}
@media (max-width: 767px) {
    .rfd-table {
        table-layout: auto !important;
    }
}
@media (max-width: 639px) {
    .rfd-toolbar {
        flex-wrap: wrap;
    }
    .rfd-filters {
        width: 100%;
        overflow-x: auto;
        -webkit-overflow-scrolling: touch;
    }
    .rfd-filter {
        font-size: 0.8rem;
        padding: 0.2rem 0.5rem;
        white-space: nowrap;
    }
    .rfd-search-wrap {
        flex: 1;
        min-width: 0;
        width: auto;
    }
    .rfd-search {
        font-size: 1rem;
    }
    .rfd-col-category {
        display: none;
    }
    .rfd-badge {
        font-size: 0.75rem;
        padding: 0.1rem 0.4rem;
    }
}
</style>
