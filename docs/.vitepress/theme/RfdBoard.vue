<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'

import { normalizePriority } from '../loaders/rfd-priority.mjs'

const props = defineProps({
    entries: { type: Array, required: true },
    // Raw board layout, in priority.json's shape: `planned` milestone groups,
    // `backlog`, and `in_development`.
    priority: { type: Object, required: true },
})

// Dragging only exists on the dev server. SortableJS is dynamically imported
// behind this flag (see onMounted), so the production bundle never includes it
// and the list renders read-only — the client-facing view.
const isDev = import.meta.env.DEV

const TERMINAL = new Set(['Implemented', 'Superseded', 'Abandoned'])

// Marker rows are synthetic list entries: labeled milestone lines plus the
// unnamed cutoff between the prioritised list and the unsorted backlog. An
// RFD belongs to the nearest milestone marker below it; anything below the
// cutoff is backlog. Markers exist only in the rendered board — the file
// stores the same structure as `planned` groups.
const CUTOFF_NUM = '--cutoff--'

function milestoneRow(name) {
    return { marker: true, name, num: `milestone:${name}`, slug: `milestone:${name}` }
}

function cutoffRow() {
    return { marker: true, name: null, num: CUTOFF_NUM, slug: CUTOFF_NUM }
}

const byNum = new Map(props.entries.map(e => [e.num, e]))

// Every RFD's hard dependencies (Requires ∪ Extends). A dependency must sit
// above its dependent in the list.
const depMap = computed(() => {
    const m = new Map()
    for (const e of props.entries) m.set(e.num, new Set(e.dependsOn ?? []))
    return m
})

// Order the active backlog with dependencies kept above their dependents. Saved
// order is respected as a tiebreak; an order that violates a dependency is shown
// corrected and persists on the next drag. The build forbids cycles, so the
// trailing concat never runs in practice.
function topoSort(list) {
    const onBoard = new Set(list.map(e => e.num))
    const deps = new Map(
        list.map(e => [e.num, (e.dependsOn ?? []).filter(d => onBoard.has(d))])
    )
    const placed = new Set()
    const result = []
    const remaining = list.slice()
    let progressed = true
    while (remaining.length && progressed) {
        progressed = false
        for (let i = 0; i < remaining.length; i++) {
            if (deps.get(remaining[i].num).every(d => placed.has(d))) {
                const [e] = remaining.splice(i, 1)
                result.push(e)
                placed.add(e.num)
                progressed = true
                i--
            }
        }
    }
    return result.concat(remaining)
}

// Build the board rows from a normalized priority record: each planned
// group's RFDs followed by its milestone marker, then the cutoff, the
// backlog, and finally any active RFDs the file doesn't mention. Dependencies
// are kept above their dependents; the saved order is the tiebreak.
function buildRows(p) {
    const isActive = e => e && !TERMINAL.has(e.status)
    const placed = new Set()
    const rows = []
    const push = num => {
        const e = byNum.get(num)
        if (isActive(e) && !placed.has(num)) {
            rows.push(e)
            placed.add(num)
        }
    }
    for (const group of p.planned) {
        group.ids.forEach(push)
        if (group.milestone !== null) rows.push(milestoneRow(group.milestone))
    }
    rows.push(cutoffRow())
    p.backlog.forEach(push)
    const rest = props.entries
        .filter(e => isActive(e) && !placed.has(e.num))
        .sort((a, b) => a.num.localeCompare(b.num))
    rows.push(...rest)
    return topoSort(rows)
}

const items = ref([])
const inDev = ref(new Set())
// Initial board state from the build-time data. On the dev server the same
// function rebuilds it from a fresh fetch on mount.
applyPriority(props.priority)

// RFD numbers currently on the board (marker rows excluded).
const rfdNums = computed(() => new Set(
    items.value.filter(r => !r.marker).map(r => r.num)
))

const cutoffIndex = computed(() =>
    items.value.findIndex(r => r.marker && r.name === null)
)

const statusClass = status => 'rfd-badge--' + (status?.toLowerCase() ?? 'unknown')

// Dev renders the whole list, including the draggable markers. The production
// build shows the prioritised rows and their milestone lines above the
// cutoff.
const visibleItems = computed(() => {
    if (isDev) return items.value
    const i = cutoffIndex.value
    return i === -1 ? items.value : items.value.slice(0, i)
})

// Dependencies of the hovered (or dragged) RFD, highlighted so its constraints
// are visible. Set on hover and on drag start; hover updates are ignored while a
// drag is in progress.
const requiredSet = ref(new Set())
// The RFD currently being dragged, or null. Drives the "required by" pills,
// which appear only during a drag.
const dragNum = ref(null)

function depsOnBoard(num) {
    return [...(depMap.value.get(num) ?? [])].filter(d => rfdNums.value.has(d))
}

function hover(num) {
    if (dragNum.value !== null) return
    requiredSet.value = new Set(depsOnBoard(num))
}

function unhover() {
    if (dragNum.value !== null) return
    requiredSet.value = new Set()
}

// Transient status note shown in the sticky bar; falls back to the hint when
// null. `kind` selects the colour (warn / ok / err). `ms = 0` keeps it until
// explicitly cleared, used for the live "blocked" note while dragging.
const notice = ref(null)
let noticeTimer = null
function setNotice(text, kind, ms = 0) {
    notice.value = { text, kind }
    if (noticeTimer) { clearTimeout(noticeTimer); noticeTimer = null }
    if (ms > 0) noticeTimer = setTimeout(() => { notice.value = null }, ms)
}
function clearNotice() {
    if (noticeTimer) { clearTimeout(noticeTimer); noticeTimer = null }
    notice.value = null
}

// First dependency violated by a candidate order, top-to-bottom: a dependency
// sitting below its dependent. Used to reject an invalid drop.
function firstViolation(order) {
    const pos = new Map(order.map((num, i) => [num, i]))
    for (const num of order) {
        for (const dep of depMap.value.get(num) ?? []) {
            if (pos.has(dep) && pos.get(dep) > pos.get(num)) {
                return { dependent: num, dependency: dep }
            }
        }
    }
    return null
}

// Marker placement violated by a candidate order: a milestone marker sitting
// below the unsorted cutoff. Milestones section the prioritised list; the
// backlog is unsorted by definition. Returns a notice message, or null.
function markerViolation(order) {
    const cut = order.indexOf(CUTOFF_NUM)
    if (cut === -1) return null
    for (let i = cut + 1; i < order.length; i++) {
        if (order[i].startsWith('milestone:')) {
            return 'Milestone markers must stay above the unsorted cutoff.'
        }
    }
    return null
}

// --- Autosave ---
async function save() {
    // Split the combined list at its markers: each milestone marker closes
    // the group of rows above it, and the cutoff separates the prioritised
    // list from the backlog. Marker rows themselves are dropped; priority.json
    // holds only RFD ids and milestone names.
    const rows = items.value
    const planned = []
    let ids = []
    let cut = 0
    for (; cut < rows.length; cut++) {
        const row = rows[cut]
        if (row.marker && row.name === null) break
        if (row.marker) {
            planned.push({ milestone: row.name, ids })
            ids = []
        } else {
            ids.push(row.num)
        }
    }
    planned.push({ milestone: null, ids })
    const body = {
        planned,
        backlog: rows.slice(cut + 1).filter(r => !r.marker).map(r => r.num),
        in_development: [...inDev.value].sort(),
    }
    try {
        const res = await fetch('/__rfd-priority', {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(body),
        })
        if (!res.ok) throw new Error(await res.text())
        setNotice('Saved', 'ok', 2000)
    } catch (err) {
        setNotice(String(err.message || err), 'err', 6000)
    }
}

function toggleDev(num) {
    const next = new Set(inDev.value)
    if (next.has(num)) next.delete(num)
    else next.add(num)
    inDev.value = next
    save()
}

// Briefly highlight a row after a jump, so it's clear where it landed.
const flashNum = ref(null)
let flashTimer = null
function flash(num) {
    flashNum.value = num
    if (flashTimer) clearTimeout(flashTimer)
    flashTimer = setTimeout(() => { flashNum.value = null }, 700)
}

// Jump a row to the top or bottom of its own section: the block bounded by
// the nearest marker rows (milestone lines or the cutoff) around it. Clamped
// so dependencies stay above the row and dependents stay below it.
function moveTo(rfd, edge) {
    const list = items.value.slice()
    const from = list.indexOf(rfd)
    if (from === -1) return

    list.splice(from, 1)
    // Section bounds after removal: a marker index below `from` is unchanged,
    // one at or past it shifted up by one — so `i < from` still identifies
    // the markers that sat above the row.
    let lo = 0
    let hi = list.length
    list.forEach((e, i) => {
        if (!e.marker) return
        if (i < from) lo = Math.max(lo, i + 1)
        else hi = Math.min(hi, i)
    })

    // Dependencies must stay above the row, dependents below it.
    const deps = new Set(depsOnBoard(rfd.num))
    let maxDep = -1
    let minDependent = list.length
    list.forEach((e, i) => {
        if (deps.has(e.num)) maxDep = Math.max(maxDep, i)
        if (depsOnBoard(e.num).includes(rfd.num)) minDependent = Math.min(minDependent, i)
    })

    let target = edge === 'top' ? Math.max(lo, maxDep + 1) : Math.min(hi, minDependent)
    target = Math.max(lo, Math.min(target, hi))
    list.splice(target, 0, rfd)

    const order = list.map(e => e.num)
    if (order.join('|') === items.value.map(e => e.num).join('|')) return
    if (firstViolation(order)) {
        setNotice('Dependencies prevent moving it that far.', 'warn', 3000)
        return
    }
    items.value = list
    flash(rfd.num)
    save()
}

// --- Milestone CRUD (dev only) ---
//
// Creating: hovering the gap above a row reveals a "+ milestone" pill after a
// short delay; clicking it opens an inline name input at that position.
// Renaming: click a milestone label. Deleting: the trashcan on the marker
// row. Deleting is non-destructive — the rows above the marker flow into the
// next milestone below, or become unassigned.

// Insertion index for a new milestone marker, or null when no input is open.
const creating = ref(null)
const createName = ref('')
// `num` of the milestone marker being renamed, or null.
const renaming = ref(null)
const renameName = ref('')

// Focus an inline input as soon as it mounts (used as a function ref).
const focusEl = el => { if (el) el.focus() }

function milestoneNames() {
    return new Set(
        items.value.filter(r => r.marker && r.name !== null).map(r => r.name)
    )
}

function openCreate(index) {
    renaming.value = null
    creating.value = index
    createName.value = ''
}

function commitCreate() {
    const index = creating.value
    const name = createName.value.trim()
    creating.value = null
    if (index === null || !name) return
    if (milestoneNames().has(name)) {
        setNotice(`Milestone "${name}" already exists.`, 'warn', 3000)
        return
    }
    const list = items.value.slice()
    list.splice(index, 0, milestoneRow(name))
    items.value = list
    flash(`milestone:${name}`)
    save()
}

function openRename(row) {
    creating.value = null
    renaming.value = row.num
    renameName.value = row.name
}

function commitRename(row) {
    const name = renameName.value.trim()
    renaming.value = null
    if (!name || name === row.name) return
    if (milestoneNames().has(name)) {
        setNotice(`Milestone "${name}" already exists.`, 'warn', 3000)
        return
    }
    const index = items.value.indexOf(row)
    if (index === -1) return
    const list = items.value.slice()
    list.splice(index, 1, milestoneRow(name))
    items.value = list
    flash(`milestone:${name}`)
    save()
}

function removeMilestone(row) {
    const index = items.value.indexOf(row)
    if (index === -1) return
    const list = items.value.slice()
    list.splice(index, 1)
    items.value = list
    save()
}

// --- Drag (dev only) ---
const listRef = ref(null)
let sortable = null

function onStart(evt) {
    clearNotice()
    const num = evt.item?.dataset?.num
    dragNum.value = num
    requiredSet.value = new Set(depsOnBoard(num))
}

// Block a move live, before it commits, by validating the candidate order — not
// just the crossed neighbour. A fast drag can skip past the dependency, so a
// pairwise check against `related` alone lets it through.
function onMove(evt) {
    const draggedNum = evt.dragged?.dataset?.num
    const relatedNum = evt.related?.dataset?.num
    if (!draggedNum || !relatedNum) return true

    const candidate = sortable.toArray().filter(n => n !== draggedNum)
    const relIdx = candidate.indexOf(relatedNum)
    if (relIdx === -1) return true
    candidate.splice(evt.willInsertAfter ? relIdx + 1 : relIdx, 0, draggedNum)

    const markerErr = markerViolation(candidate)
    if (markerErr) {
        setNotice(markerErr, 'warn')
        return false
    }

    const pos = new Map(candidate.map((n, i) => [n, i]))
    const di = pos.get(draggedNum)

    // A dependency would end up below the dragged RFD.
    for (const dep of depsOnBoard(draggedNum)) {
        if (pos.get(dep) > di) {
            setNotice(`RFD ${draggedNum} requires RFD ${dep}, which must stay above it.`, 'warn')
            return false
        }
    }
    // A dependent would end up above the dragged RFD.
    for (const [other, oi] of pos) {
        if (oi < di && depsOnBoard(other).includes(draggedNum)) {
            setNotice(`RFD ${other} requires RFD ${draggedNum}, which must stay above it.`, 'warn')
            return false
        }
    }

    clearNotice()
    return true
}

function onEnd() {
    dragNum.value = null
    requiredSet.value = new Set()

    const order = sortable.toArray()
    const current = items.value.map(e => e.num)
    if (order.join('|') === current.join('|')) {
        clearNotice()
        return
    }

    // Revert the DOM to Vue's order so Vue stays the single source of truth,
    // then drive the change through the reactive array.
    sortable.sort(current, false)

    const markerErr = markerViolation(order)
    if (markerErr) {
        setNotice(`${markerErr} Reorder reverted.`, 'warn', 3000)
        return
    }

    const violation = firstViolation(order)
    if (violation) {
        setNotice(
            `RFD ${violation.dependent} requires RFD ${violation.dependency}, ` +
            `which must stay above it. Reorder reverted.`,
            'warn',
            3000
        )
        return
    }

    clearNotice()
    const rowByNum = new Map(items.value.map(e => [e.num, e]))
    items.value = order.map(num => rowByNum.get(num))
    save()
}

// Rebuild the board from a raw priority record (priority.json's shape),
// tolerating the legacy flat `order` format. Runs at setup with the
// build-time data, and again on mount with a fresh fetch: in dev the
// VitePress data loader caches its result for the server's lifetime, so a
// refresh would otherwise show stale priorities. Production has no endpoint
// and keeps the build-time data (correct for the static site).
function applyPriority(raw) {
    const p = normalizePriority(raw)
    items.value = buildRows(p)
    const onBoard = new Set(
        items.value.filter(r => !r.marker).map(r => r.num)
    )
    inDev.value = new Set(p.inDevelopment.filter(num => onBoard.has(num)))
}

async function loadFreshPriority() {
    try {
        const res = await fetch('/__rfd-priority')
        if (res.ok) applyPriority(await res.json())
    } catch { /* keep the build-time data */ }
}

onMounted(() => {
    if (!isDev) return
    loadFreshPriority()
    import('sortablejs').then(({ default: Sortable }) => {
        if (!listRef.value) return
        sortable = Sortable.create(listRef.value, {
            dataIdAttr: 'data-num',
            animation: 150,
            // On touch, hold briefly to drag so a quick swipe still scrolls the
            // page. On desktop, dragging starts immediately.
            delay: 150,
            delayOnTouchOnly: true,
            touchStartThreshold: 5,
            // Drag from anywhere on the row, except the link, dev toggle, and
            // jump buttons, which stay clickable.
            filter: 'a, input, label, button, .rfd-board-addzone',
            preventOnFilter: false,
            ghostClass: 'rfd-board-ghost',
            chosenClass: 'rfd-board-chosen',
            dragClass: 'rfd-board-drag',
            onStart,
            onMove,
            onEnd,
        })
    })
})

onBeforeUnmount(() => {
    if (sortable) {
        sortable.destroy()
        sortable = null
    }
})
</script>

<template>
<div class="rfd-board">
    <div v-if="isDev" class="rfd-board-status">
        <span v-if="notice" :class="'rfd-board-' + notice.kind">{{ notice.text }}</span>
        <span v-else class="rfd-board-hint">Drag a row to reorder; hover the gap between rows to add a milestone. Changes save automatically.</span>
    </div>
    <p v-else class="rfd-board-note">
        The active backlog in priority order. Top of the list is worked on first.
        Every RFD above a milestone line targets that release.
    </p>

    <ol ref="listRef" class="rfd-board-list" :class="{ 'is-editable': isDev }">
        <li
            v-for="(rfd, idx) in visibleItems"
            :key="rfd.slug"
            :data-num="rfd.num"
            class="rfd-board-item"
            :class="{
                'is-dev': inDev.has(rfd.num),
                'is-required': requiredSet.has(rfd.num),
                'is-flash': flashNum === rfd.num,
                'rfd-board-cutoff': rfd.marker,
            }"
            @mouseenter="hover(rfd.num)"
            @mouseleave="unhover()"
        >
            <span
                v-if="isDev && idx <= cutoffIndex"
                class="rfd-board-addzone"
                :class="{ 'is-open': creating === idx }"
                @click.stop="openCreate(idx)"
            >
                <input
                    v-if="creating === idx"
                    :ref="focusEl"
                    v-model="createName"
                    class="rfd-board-milestone-input"
                    placeholder="milestone name"
                    @click.stop
                    @keydown.enter.prevent="commitCreate()"
                    @keydown.esc.prevent="creating = null"
                    @blur="creating = null"
                />
                <span v-else class="rfd-board-addzone-pill">+ milestone</span>
            </span>
            <template v-if="rfd.marker">
                <span class="rfd-board-cutoff-line"></span>
                <span v-if="rfd.name === null" class="rfd-board-cutoff-label">unsorted below</span>
                <template v-else>
                    <input
                        v-if="isDev && renaming === rfd.num"
                        :ref="focusEl"
                        v-model="renameName"
                        class="rfd-board-milestone-input"
                        @click.stop
                        @keydown.enter.prevent="commitRename(rfd)"
                        @keydown.esc.prevent="renaming = null"
                        @blur="renaming = null"
                    />
                    <span
                        v-else
                        class="rfd-board-milestone-label"
                        :class="{ 'is-editable': isDev }"
                        :title="isDev ? 'Click to rename' : undefined"
                        @click="isDev && openRename(rfd)"
                    >{{ rfd.name }} milestone</span>
                    <button
                        v-if="isDev && renaming !== rfd.num"
                        class="rfd-board-trash"
                        title="Remove milestone"
                        @click.stop="removeMilestone(rfd)"
                    >🗑</button>
                </template>
                <span class="rfd-board-cutoff-line"></span>
            </template>
            <template v-else>
                <span v-if="isDev" class="rfd-board-handle" aria-hidden="true" title="Drag to reorder">⠿</span>
                <div class="rfd-board-main">
                    <div class="rfd-board-titlerow">
                        <span class="rfd-board-num">{{ rfd.num }}</span>
                        <a :href="rfd.path" target="_blank" rel="noopener" class="rfd-board-title">{{ rfd.title }}</a>
                        <span v-if="inDev.has(rfd.num)" class="rfd-badge rfd-badge--indev">in development</span>
                        <span class="rfd-badge" :class="statusClass(rfd.status)">{{ rfd.status }}</span>
                        <span v-if="dragNum && requiredSet.has(rfd.num)" class="rfd-badge rfd-board-reqpill">required by RFD {{ dragNum }}</span>
                    </div>
                    <div v-if="rfd.summary" class="rfd-board-summary">{{ rfd.summary }}</div>
                </div>
                <label v-if="isDev && rfd.status !== 'Draft'" class="rfd-board-devtoggle" title="Mark as in development">
                    <input type="checkbox" :checked="inDev.has(rfd.num)" @change="toggleDev(rfd.num)" />
                    dev
                </label>
                <span v-if="isDev" class="rfd-board-jump">
                    <button class="rfd-board-jump-btn" title="Move to top of section" @click="moveTo(rfd, 'top')">▲</button>
                    <button class="rfd-board-jump-btn" title="Move to bottom of section" @click="moveTo(rfd, 'bottom')">▼</button>
                </span>
            </template>
        </li>
    </ol>
</div>
</template>

<style>
.rfd-board { margin-top: 2rem; }
.rfd-board-status {
    position: sticky;
    top: var(--vp-nav-height, 64px);
    z-index: 2;
    min-height: 1.4rem;
    margin-bottom: 1rem;
    padding: 0.5rem 0;
    background: var(--vp-c-bg);
    border-bottom: 1px solid var(--vp-c-divider);
    font-size: 0.85rem;
    color: var(--vp-c-text-3);
}
.rfd-board-ok { color: var(--vp-c-green-1); }
.rfd-board-err { color: var(--vp-c-red-1); }
.rfd-board-warn { color: var(--vp-c-yellow-1); }
.rfd-board-note { color: var(--vp-c-text-2); font-size: 0.9rem; }
.rfd-board-list { list-style: none; padding: 0; margin: 0; }
/* Trailing space inside the sortable so there's a drop zone below the last row
   to target the final position. */
.rfd-board-list.is-editable { padding-bottom: 1.5rem; }
.rfd-board-list.is-editable .rfd-board-item { cursor: grab; }
.rfd-board-list.is-editable .rfd-board-item:active { cursor: grabbing; }
.rfd-board-item {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
    padding: 0.6rem 0.75rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 8px;
    margin: 0.4rem 0;
    background: var(--vp-c-bg-soft);
    transition: background-color 0.15s;
}
/* State shows on the handle colour and a soft background tint, so the item keeps
   a clean uniform 1px frame. */
.rfd-board-item.is-dev {
    background-color: color-mix(in srgb, #14b8a6 8%, var(--vp-c-bg-soft));
}
.rfd-board-item.is-required {
    background-color: color-mix(in srgb, var(--vp-c-brand-1) 10%, var(--vp-c-bg-soft));
}
.rfd-board-item.rfd-board-cutoff {
    border: none;
    background: none;
    padding: 0.15rem 0.75rem;
    margin: 0.5rem 0;
    align-items: center;
    gap: 0.5rem;
}
.rfd-board-cutoff-line {
    flex: 1;
    height: 0;
    border-top: 1px dashed var(--vp-c-divider);
}
.rfd-board-cutoff-label {
    font-size: 0.7rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--vp-c-text-3);
    white-space: nowrap;
}
.rfd-board-milestone-label {
    font-size: 0.75rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: var(--vp-c-brand-1);
    white-space: nowrap;
}
.rfd-board-milestone-label.is-editable { cursor: pointer; }
.rfd-board-milestone-label.is-editable:hover { text-decoration: underline dotted; }
/* Trashcan on a milestone marker row, revealed after a short hover delay. The
   delay only applies on the way in; hiding is immediate. */
.rfd-board-trash {
    border: none;
    background: none;
    padding: 0;
    margin: 0;
    line-height: 1;
    font-size: 0.85rem;
    cursor: pointer;
    opacity: 0;
    transition: opacity 0.12s;
}
.rfd-board-item:hover .rfd-board-trash {
    opacity: 1;
    transition-delay: 0.5s;
}
/* Hover zone in the gap above a row (dev only): reveals the "+ milestone"
   pill after a short delay; clicking opens the inline name input. Overlaid on
   the row's top margin so the list's DOM children stay just the sortable
   rows — SortableJS counts children to compute drop indices. */
.rfd-board-addzone {
    position: absolute;
    left: 0;
    right: 0;
    top: -0.8rem;
    height: 0.8rem;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    z-index: 1;
}
.rfd-board-addzone-pill {
    font-size: 0.7rem;
    line-height: 1;
    padding: 0.15rem 0.5rem;
    border: 1px dashed var(--vp-c-brand-1);
    border-radius: 999px;
    color: var(--vp-c-brand-1);
    background: var(--vp-c-bg);
    opacity: 0;
    transition: opacity 0.12s;
}
.rfd-board-addzone:hover .rfd-board-addzone-pill {
    opacity: 1;
    transition-delay: 0.5s;
}
.rfd-board-milestone-input {
    position: relative;
    z-index: 2;
    font-size: 0.75rem;
    line-height: 1.2;
    padding: 0.15rem 0.4rem;
    border: 1px solid var(--vp-c-brand-1);
    border-radius: 4px;
    background: var(--vp-c-bg);
    color: var(--vp-c-text-1);
    width: 10rem;
}
.rfd-board-handle {
    align-self: stretch;
    display: flex;
    align-items: center;
    justify-content: center;
    width: 1.4rem;
    font-size: 1.5rem;
    line-height: 1;
    color: color-mix(in srgb, var(--vp-c-text-3) 50%, transparent);
    cursor: grab;
    user-select: none;
    touch-action: none;
    transition: color 0.15s;
}
.rfd-board-handle:active { cursor: grabbing; }
.rfd-board-item.is-dev .rfd-board-handle { color: #0d9488; }
.rfd-board-item.is-required .rfd-board-handle { color: var(--vp-c-brand-1); }
/* One-click jumps to the top/bottom of the row's section. Positioned in the
   margin to the left of the row, outside its box, revealed on hover.
   `padding-right` bridges the gap to the row so the hover stays contiguous as
   the cursor moves onto the arrows. */
.rfd-board-jump {
    position: absolute;
    right: 100%;
    top: 0;
    bottom: 0;
    /* Wide box for an easy hover target; the arrows stay right-aligned so they
       sit close to the row, with padding-right as the gap. */
    width: 2.5rem;
    padding-right: 0.4rem;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    justify-content: center;
    gap: 1px;
    opacity: 0;
    transition: opacity 0.12s;
}
.rfd-board-item:hover .rfd-board-jump,
.rfd-board-jump:hover {
    opacity: 1;
}
.rfd-board-jump-btn {
    border: none;
    background: none;
    padding: 0;
    margin: 0;
    line-height: 1;
    font-size: 0.7rem;
    color: var(--vp-c-text-3);
    cursor: pointer;
}
.rfd-board-jump-btn:hover { color: var(--vp-c-brand-1); }
@keyframes rfd-board-flash {
    from { box-shadow: 0 0 0 3px color-mix(in srgb, var(--vp-c-brand-1) 60%, transparent); }
    to { box-shadow: 0 0 0 3px transparent; }
}
.rfd-board-item.is-flash { animation: rfd-board-flash 0.7s ease; }
.rfd-board-reqpill {
    background: color-mix(in srgb, var(--vp-c-brand-1) 20%, transparent);
    color: var(--vp-c-brand-1);
}
.rfd-board-ghost { opacity: 0.4; }
.rfd-board-drag { opacity: 0; }
.rfd-board-num {
    font-variant-numeric: tabular-nums;
    color: var(--vp-c-text-2);
    font-size: 0.85rem;
    min-width: 2.2rem;
}
.rfd-board-main { flex: 1; min-width: 0; }
.rfd-board-titlerow {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
}
.rfd-board-title { font-weight: 500; }
.rfd-board-summary {
    font-size: 0.8rem;
    color: var(--vp-c-text-2);
    line-height: 1.4;
    margin-top: 0.15rem;
    /* Align under the title: past the number gutter (min-width) + titlerow gap. */
    margin-left: 2.7rem;
}
.rfd-board-devtoggle {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.75rem;
    color: var(--vp-c-text-3);
    cursor: pointer;
    white-space: nowrap;
}
</style>
