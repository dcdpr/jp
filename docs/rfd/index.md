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
    cols.push({ key: 'date', label: 'Date' })
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

const filtered = computed(() => {
    let rows = filter.value === 'all'
        ? [...data]
        : data.filter(r => r.category?.toLowerCase() === filter.value)

    const q = search.value.trim().toLowerCase()
    if (q) {
        rows = rows.filter(r =>
            [r.title, r.category, r.status, r.date, r.summary]
                .some(v => v?.toLowerCase().includes(q))
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
    <input
        v-model="search"
        class="rfd-search"
        type="text"
        placeholder="Filter…"
    />
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
    <col v-if="showCategory" style="width: 7rem">
    <col style="width: 8rem">
    <col style="width: 8rem">
</colgroup>
<thead><tr>
    <th v-for="col in columns" :key="col.key" class="rfd-sortable" @click="toggleSort(col.key)">
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
    <td v-if="showCategory">{{ rfd.category }}</td>
    <td><span :class="'rfd-badge rfd-badge--' + (rfd.status?.toLowerCase() ?? 'unknown')">{{ rfd.status }}</span></td>
    <td>{{ rfd.date }}</td>
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
.rfd-search {
    padding: 0.3rem 0.75rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 4px;
    background: transparent;
    color: var(--vp-c-text-1);
    font-size: 0.9rem;
    outline: none;
    width: 14rem;
}
.rfd-search::placeholder {
    color: var(--vp-c-text-3);
}
.rfd-search:focus {
    border-color: var(--vp-c-brand-1);
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
.rfd-badge {
    display: inline-block;
    padding: 0.1rem 0.55rem;
    border-radius: 9999px;
    font-size: 0.8rem;
    font-weight: 500;
    line-height: 1.4;
    white-space: nowrap;
}
.rfd-badge--implemented {
    background: color-mix(in srgb, #10b981 20%, transparent);
    color: #059669;
}
.rfd-badge--accepted {
    background: color-mix(in srgb, #3b82f6 20%, transparent);
    color: #2563eb;
}
.rfd-badge--discussion {
    background: color-mix(in srgb, #a855f7 20%, transparent);
    color: #7c3aed;
}
.rfd-badge--draft {
    background: color-mix(in srgb, #6b7280 20%, transparent);
    color: #4b5563;
}
.rfd-badge--superseded {
    background: color-mix(in srgb, #f59e0b 20%, transparent);
    color: #d97706;
}
.rfd-badge--abandoned {
    background: color-mix(in srgb, #ef4444 20%, transparent);
    color: #dc2626;
}
.dark .rfd-badge--implemented {
    background: color-mix(in srgb, #10b981 25%, transparent);
    color: #6ee7b7;
}
.dark .rfd-badge--accepted {
    background: color-mix(in srgb, #3b82f6 25%, transparent);
    color: #93c5fd;
}
.dark .rfd-badge--discussion {
    background: color-mix(in srgb, #a855f7 25%, transparent);
    color: #d8b4fe;
}
.dark .rfd-badge--draft {
    background: color-mix(in srgb, #6b7280 25%, transparent);
    color: #d1d5db;
}
.dark .rfd-badge--superseded {
    background: color-mix(in srgb, #f59e0b 25%, transparent);
    color: #fcd34d;
}
.dark .rfd-badge--abandoned {
    background: color-mix(in srgb, #ef4444 25%, transparent);
    color: #fca5a5;
}
</style>
