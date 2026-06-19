<script setup>
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'

const props = defineProps({
    entries: { type: Array, required: true },
})

// Dragging only exists on the dev server. SortableJS is dynamically imported
// behind this flag (see onMounted), so the production bundle never includes it
// and the list renders read-only — the client-facing view.
const isDev = import.meta.env.DEV

const TERMINAL = new Set(['Implemented', 'Superseded', 'Abandoned'])

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

const active = props.entries
    .filter(e => !TERMINAL.has(e.status))
    .sort((a, b) => {
        const ap = a.priority ?? Infinity
        const bp = b.priority ?? Infinity
        if (ap !== bp) return ap - bp
        return a.num.localeCompare(b.num)
    })

const items = ref(topoSort(active))
const byNum = new Map(props.entries.map(e => [e.num, e]))
const activeNums = new Set(items.value.map(e => e.num))

const inDev = ref(new Set(
    props.entries
        .filter(e => e.inDevelopment && activeNums.has(e.num))
        .map(e => e.num)
))

const statusClass = status => 'rfd-badge--' + (status?.toLowerCase() ?? 'unknown')

// Dependencies of the hovered (or dragged) RFD, highlighted so its constraints
// are visible. Set on hover and on drag start; hover updates are ignored while a
// drag is in progress.
const requiredSet = ref(new Set())
// The RFD currently being dragged, or null. Drives the "required by" pills,
// which appear only during a drag.
const dragNum = ref(null)

function depsOnBoard(num) {
    return [...(depMap.value.get(num) ?? [])].filter(d => activeNums.has(d))
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

// --- Autosave ---
async function save() {
    const body = {
        order: items.value.map(e => e.num),
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
    items.value = order.map(num => byNum.get(num))
    save()
}

onMounted(() => {
    if (!isDev) return
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
            // Drag from anywhere on the row, except the link and the dev toggle,
            // which stay clickable.
            filter: 'a, input, label',
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
        <span v-else class="rfd-board-hint">Drag a row to reorder — changes save automatically.</span>
    </div>
    <p v-else class="rfd-board-note">
        The active backlog in priority order. Top of the list is worked on first.
    </p>

    <ol ref="listRef" class="rfd-board-list" :class="{ 'is-editable': isDev }">
        <li
            v-for="rfd in items"
            :key="rfd.slug"
            :data-num="rfd.num"
            class="rfd-board-item"
            :class="{
                'is-dev': inDev.has(rfd.num),
                'is-required': requiredSet.has(rfd.num),
            }"
            @mouseenter="hover(rfd.num)"
            @mouseleave="unhover()"
        >
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
