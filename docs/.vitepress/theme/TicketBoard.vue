<script setup>
import { onBeforeUnmount, onMounted, ref } from 'vue'

import { createSortable, isDev, loadBoard, saveBoard } from './board.mjs'

const props = defineProps({
    // Columns in flow order, each already ordered and (for Done) capped.
    columns: { type: Array, required: true },
})

const ENDPOINT = '/__ticket-board'
const LABELS = { todo: 'Todo', in_progress: 'In Progress', done: 'Done' }

// Cards move between columns as well as within them, so all three lists join
// one drag group.
const GROUP = 'ticket-board'

const columns = ref(props.columns.map(column => ({ ...column, tickets: [...column.tickets] })))
const listRefs = ref([])
const notice = ref(null)
let sortables = []
let noticeTimer = null

function setNotice(text, kind, ms) {
    notice.value = { text, kind }
    clearTimeout(noticeTimer)
    noticeTimer = setTimeout(() => { notice.value = null }, ms)
}

function kindClass(kind) {
    return `ticket-kind--${(kind ?? 'unknown').toLowerCase()}`
}

/// Rebuild the model from what the DOM now says, then persist it.
///
/// SortableJS moves the nodes; `toArray()` reads the resulting order back out
/// via each card's `data-id`. The model follows the DOM rather than the other
/// way round, which keeps Vue from fighting the drag.
async function onDrop() {
    const byId = new Map(
        columns.value.flatMap(column => column.tickets).map(ticket => [ticket.id, ticket]))

    const next = columns.value.map((column, index) => {
        const ids = sortables[index]?.toArray() ?? column.tickets.map(t => t.id)
        const tickets = ids
            .map(id => byId.get(id))
            .filter(Boolean)
            // A card that crossed a column takes that column's status with it.
            .map(ticket => ({ ...ticket, status: column.status }))

        return { ...column, tickets, total: tickets.length }
    })
    columns.value = next

    const body = Object.fromEntries(
        next.map(column => [column.key, column.tickets.map(ticket => ticket.id)]))

    const error = await saveBoard(ENDPOINT, body)
    setNotice(error ?? 'Saved', error ? 'err' : 'ok', error ? 6000 : 2000)
}

onMounted(async () => {
    if (!isDev) return

    // The board file carries order only; statuses live in the ticket files. A
    // fresh read picks up both, since the loader's cache is per server run.
    const fresh = await loadBoard(ENDPOINT)
    if (fresh?.columns) {
        columns.value = fresh.columns.map(column => ({ ...column, tickets: [...column.tickets] }))
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
<div class="ticket-board">
    <section v-for="(column, index) in columns" :key="column.key" class="ticket-column">
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
                    <span class="ticket-badge" :class="kindClass(ticket.kind)">{{ ticket.kind }}</span>
                </div>
                <a :href="ticket.path" class="ticket-card-title">{{ ticket.title }}</a>
                <div class="ticket-card-foot">
                    <span v-if="ticket.blockedBy" class="ticket-badge doc-badge--blocked">
                        blocked by {{ ticket.blockedBy }}
                    </span>
                    <span v-if="ticket.implements" class="ticket-meta">
                        implements RFD {{ ticket.implements }}
                    </span>
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
.ticket-kind--bug {
    background: var(--vp-c-danger-soft);
    color: var(--vp-c-danger-1);
}
.ticket-kind--feature {
    background: var(--vp-c-brand-soft);
    color: var(--vp-c-brand-1);
}
.ticket-kind--chore {
    background: var(--vp-c-default-soft);
    color: var(--vp-c-text-2);
}
@media (max-width: 767px) {
    .ticket-board {
        grid-template-columns: minmax(0, 1fr);
    }
}
</style>
