<script setup>
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'

import { data } from '../loaders/tickets.data.js'
import { createSortable, isDev, loadBoard, saveBoard } from './board.mjs'
import DocLabelFilter from './DocLabelFilter.vue'
import DocLabels from './DocLabels.vue'
import { matchesLabelFilter } from './label-filter.mjs'
import { problem } from './problem.mjs'
import { childrenOf, inGroup, progress } from './ticket-tree.mjs'
import { useLabelFilter } from './use-label-filter.mjs'
import { useParentFilter } from './use-parent-filter.mjs'

// The ticket board, which a page renders with `<TicketBoard />`.
//
// `data.columns` holds the columns in flow order, each ordered and complete;
// the board shows the first `data.doneLimit` tickets of Done.
//
// `?label=` and `?any-label=` narrow the board to the tickets carrying those
// labels (see `label-filter.mjs`), and a narrowed board shows the whole of
// Done. Clicking a card's label narrows the board to it, and clicking it again
// takes it back out; see `DocLabels.vue`.
//
// `?under=` narrows the board to one parent and the tickets filed under it
// (see `ticket-tree.mjs`), alongside any label filter. A child's card names its
// parent, and a parent's card counts its children done; clicking either
// narrows the board to the group, and clicking it again widens it back.

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
const { filter, toggle, clear } = useLabelFilter()
const { parent, toggle: toggleParent, clear: clearParent } = useParentFilter()

// Every ticket on the board, for finding a parent's children and title.
const everyTicket = computed(() => columns.value.flatMap(column => column.tickets))

// How many of each parent's children are done, by the parent's id.
const families = computed(() => {
    const counted = new Map()
    for (const ticket of everyTicket.value) {
        const children = childrenOf(everyTicket.value, ticket.id)
        if (children.length > 0) counted.set(ticket.id, progress(children))
    }
    return counted
})

// The parent the board is narrowed to, as a ticket, when it is one.
const parentTicket = computed(() => everyTicket.value.find(ticket => ticket.id === parent.value) ?? null)

// The columns as the board shows them: the tickets the filters let through,
// or, with no filter, Done cut to its limit. `total` counts what passed.
const shown = computed(() => columns.value.map(column => {
    const passing = column.tickets.filter(ticket =>
        matchesLabelFilter(ticket.labels ?? [], filter.value) && inGroup(ticket, parent.value))
    const narrowed = filter.value || parent.value
    const cut = !narrowed && column.key === 'done' ? passing.slice(0, doneLimit.value) : passing

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
        // without movement still follows the link. A label is for filtering,
        // so it starts no drag.
        filter: 'input, label, button, .doc-labels',
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
<DocLabelFilter v-if="!problem" :filter="filter" noun="Tickets" :clear="clear" />
<p v-if="!problem && parent" class="doc-label-filter">
    {{ parent }}{{ parentTicket ? `, ${parentTicket.title},` : '' }} and the tickets under it.
    <button class="doc-label-filter-clear" @click="clearParent()">Show every group</button>
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
                    <button
                        v-if="ticket.parent"
                        class="ticket-group"
                        :class="{ 'is-active': parent === ticket.parent }"
                        :title="`Show only ${ticket.parent} and the tickets under it`"
                        @click="toggleParent(ticket.parent)"
                    >under {{ ticket.parent }}</button>
                    <button
                        v-if="families.has(ticket.id)"
                        class="ticket-group"
                        :class="{ 'is-active': parent === ticket.id }"
                        :title="`Show only ${ticket.id} and the tickets under it`"
                        @click="toggleParent(ticket.id)"
                    >{{ families.get(ticket.id).done }}/{{ families.get(ticket.id).total }} done</button>
                </div>
                <a :href="ticket.path" class="ticket-card-title">{{ ticket.title }}</a>
                <DocLabels
                    v-if="ticket.labels?.length"
                    :labels="ticket.labels"
                    :filter="filter"
                    :pick="toggle"
                />
                <div v-if="ticket.comments || ticket.checklist" class="ticket-meta">
                    <span v-if="ticket.checklist">
                        {{ ticket.checklist.checked }} of {{ ticket.checklist.count }} checked
                    </span>
                    <span v-if="ticket.comments">
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
.ticket-group {
    padding: 0 0.35rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 4px;
    background: transparent;
    font-family: var(--vp-font-family-mono);
    font-size: 0.7rem;
    color: var(--vp-c-text-2);
    cursor: pointer;
}
.ticket-group:hover,
.ticket-group.is-active {
    border-color: var(--vp-c-brand-1);
    color: var(--vp-c-brand-1);
}
.ticket-card-title {
    display: block;
    font-size: 0.9rem;
    line-height: 1.35;
    font-weight: 500;
}
.ticket-meta {
    display: flex;
    gap: 0.75rem;
    margin-top: 0.25rem;
    font-size: 0.75rem;
    color: var(--vp-c-text-3);
}
.ticket-empty {
    font-size: 0.8rem;
    color: var(--vp-c-text-3);
    margin: 0.25rem 0 0 !important;
}
@media (max-width: 767px) {
    .ticket-board {
        grid-template-columns: minmax(0, 1fr);
    }
}
</style>
