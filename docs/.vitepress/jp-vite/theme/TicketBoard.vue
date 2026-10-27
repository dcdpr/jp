<script setup>
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'

import { data } from '../loaders/tickets.data.js'
import { createSortable, isDev, loadBoard, saveBoard } from './board.mjs'
import { describeLabelFilter, matchesLabelFilter, parseLabelFilter } from './label-filter.mjs'
import { problem } from './problem.mjs'

// The ticket board, which a page renders with `<TicketBoard />`.
//
// `data.columns` holds the columns in flow order, each ordered and complete;
// the board shows the first `data.doneLimit` tickets of Done.
//
// `?label=` and `?any-label=` narrow the board to the tickets carrying those
// labels (see `label-filter.mjs`), and a narrowed board shows the whole of
// Done.

const ENDPOINT = '/__ticket-board'
const LABELS = { todo: 'Todo', in_progress: 'In Progress', done: 'Done' }

// Cards move between columns as well as within them, so all three lists join
// one drag group.
const GROUP = 'ticket-board'

const copied = list => list.map(column => ({ ...column, tickets: [...column.tickets] }))

const columns = ref(copied(data.columns))
const doneLimit = ref(data.doneLimit)
const listRefs = ref([])
const notice = ref(null)
let sortables = []
let noticeTimer = null

// The page's label filter. The server renders the whole board, so the filter
// applies once the page has mounted in the browser.
const filter = ref(null)

// The columns as the board shows them: the tickets the filter lets through,
// or, with no filter, Done cut to its limit. `total` counts what passed.
const shown = computed(() => columns.value.map(column => {
    const passing = column.tickets.filter(ticket => matchesLabelFilter(ticket.labels ?? [], filter.value))
    const cut = !filter.value && column.key === 'done' ? passing.slice(0, doneLimit.value) : passing

    return { ...column, total: passing.length, tickets: cut }
}))

function setNotice(text, kind, ms) {
    notice.value = { text, kind }
    clearTimeout(noticeTimer)
    noticeTimer = setTimeout(() => { notice.value = null }, ms)
}

/// `list` with ticket `id` moved into the column at `to`, directly below
/// `after`, or, when it landed at the top, directly above `before`.
///
/// The board may show only some of a column, so the move is made in the whole
/// of it: a ticket the board isn't showing keeps its place.
function placed(list, id, to, after, before) {
    let ticket = null
    const rest = list.map(column => ({
        ...column,
        tickets: column.tickets.filter(each => {
            if (each.id !== id) return true
            ticket = each
            return false
        }),
    }))
    if (!ticket) return list

    const target = rest[to]
    const rows = [...target.tickets]
    const below = after ? rows.findIndex(each => each.id === after) : -1
    const above = before ? rows.findIndex(each => each.id === before) : -1
    const at = below >= 0 ? below + 1 : above >= 0 ? above : rows.length
    // A card that crossed a column takes that column's status with it.
    rows.splice(at, 0, { ...ticket, status: target.status })
    rest[to] = { ...target, tickets: rows }

    return rest
}

/// Make the move SortableJS just made in the DOM in the model, then save it.
///
/// `toArray()` reads the column the card landed in back out of the DOM via each
/// card's `data-id`, which gives the cards now above and below it. The model
/// takes the same order, so Vue doesn't fight the drag.
///
/// One drag is one move: the ticket, the column it landed in, and the cards
/// directly above and below it there. The dev server turns that into one
/// `jp ticket status`, placing the ticket next to a neighbour.
async function onDrop(event) {
    const moved = event.from !== event.to
    if (!moved && event.oldIndex === event.newIndex) return

    const to = sortables.findIndex(sortable => sortable?.el === event.to)
    const id = event.item?.dataset?.id
    const ids = sortables[to]?.toArray() ?? []
    const at = ids.indexOf(id)
    if (to === -1 || at === -1) return

    const after = ids[at - 1] ?? null
    const before = ids[at + 1] ?? null
    columns.value = placed(columns.value, id, to, after, before)

    const error = await saveBoard(ENDPOINT, {
        id,
        status: columns.value[to].status,
        moved,
        after,
        before,
    })
    setNotice(error ?? 'Saved', error ? 'err' : 'ok', error ? 6000 : 2000)
}

onMounted(async () => {
    filter.value = parseLabelFilter(window.location.search)
    if (!isDev) return

    // The board file carries order only; statuses live in the ticket files. A
    // fresh read picks up both, since the loader's cache is per server run.
    const fresh = await loadBoard(ENDPOINT)
    if (fresh?.columns) {
        columns.value = copied(fresh.columns)
        doneLimit.value = fresh.doneLimit ?? doneLimit.value
    }

    sortables = await Promise.all(listRefs.value.map(el => createSortable(el, {
        group: GROUP,
        dataIdAttr: 'data-id',
        // The title is most of the card, so it has to be draggable. A click
        // without movement still follows the link.
        filter: 'input, label, button',
        onEnd: onDrop,
    })))
})

onBeforeUnmount(() => {
    clearTimeout(noticeTimer)
    for (const sortable of sortables) sortable?.destroy()
    sortables = []
})
</script>

<template>
<p v-if="!problem && filter" class="ticket-board-filter">
    {{ describeLabelFilter(filter) }}.
    <a href="?">Show every ticket</a>
</p>
<div v-if="!problem" class="ticket-board">
    <section v-for="(column, index) in shown" :key="column.key" class="ticket-column">
        <h2 class="ticket-column-head">
            {{ LABELS[column.key] ?? column.status }}
            <span class="ticket-count">{{ column.total }}</span>
        </h2>

        <div :ref="el => listRefs[index] = el" class="ticket-column-list">
            <article
                v-for="ticket in column.tickets"
                :key="ticket.id"
                :data-id="ticket.id"
                class="ticket-card"
            >
                <div class="ticket-card-head">
                    <span class="ticket-id">{{ ticket.id }}</span>
                </div>
                <a :href="ticket.path" class="ticket-card-title">{{ ticket.title }}</a>
                <div class="ticket-card-foot">
                    <span
                        v-for="label in ticket.labels ?? []"
                        :key="label"
                        class="ticket-badge doc-badge--label"
                    >{{ label }}</span>
                    <span v-if="ticket.comments" class="ticket-meta">
                        {{ ticket.comments }} {{ ticket.comments === 1 ? 'comment' : 'comments' }}
                    </span>
                </div>
            </article>
        </div>

        <p v-if="column.tickets.length === 0" class="ticket-empty">Nothing here.</p>
    </section>

    <div v-if="notice" class="rfd-board-notice" :class="'is-' + notice.kind">{{ notice.text }}</div>
</div>
</template>

<style>
.ticket-board {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 1rem;
    margin-top: 2rem;
}
.ticket-board-filter {
    font-size: 0.85rem;
    color: var(--vp-c-text-2);
}
.ticket-column {
    min-width: 0;
}
.ticket-column-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0 0 0.75rem !important;
    padding: 0 0 0.35rem !important;
    border-top: none !important;
    border-bottom: 1px solid var(--vp-c-divider);
    font-size: 0.95rem !important;
    letter-spacing: 0.02em;
    text-transform: uppercase;
    color: var(--vp-c-text-2);
}
/* Keeps an empty column a droppable target. */
.ticket-column-list {
    min-height: 3rem;
}
.ticket-count {
    font-size: 0.8rem;
    font-weight: 400;
    color: var(--vp-c-text-3);
}
.ticket-card {
    border: 1px solid var(--vp-c-divider);
    border-radius: 6px;
    background: var(--vp-c-bg-soft);
    padding: 0.6rem 0.7rem;
    margin-bottom: 0.5rem;
}
.ticket-card-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-bottom: 0.25rem;
}
.ticket-id {
    font-family: var(--vp-font-family-mono);
    font-size: 0.75rem;
    color: var(--vp-c-text-3);
}
.ticket-card-title {
    display: block;
    font-size: 0.9rem;
    line-height: 1.35;
    font-weight: 500;
}
.ticket-card-foot {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem;
    margin-top: 0.35rem;
}
.ticket-meta {
    font-size: 0.75rem;
    color: var(--vp-c-text-3);
}
.ticket-empty {
    font-size: 0.8rem;
    color: var(--vp-c-text-3);
    margin: 0.25rem 0 0 !important;
}
.ticket-badge {
    display: inline-block;
    padding: 0.1rem 0.45rem;
    border-radius: 10px;
    font-size: 0.7rem;
    line-height: 1.5;
    white-space: nowrap;
    background: var(--vp-c-default-soft);
    color: var(--vp-c-text-2);
}
@media (max-width: 767px) {
    .ticket-board {
        grid-template-columns: minmax(0, 1fr);
    }
}
</style>
