<script setup>
import { useRoute } from 'vitepress'
import { computed } from 'vue'

import { data } from '../loaders/tickets.data.js'
import settings from '../settings.json'
import { documentAt } from './documents.mjs'
import { childrenOf, progress } from './ticket-tree.mjs'

// The tickets filed under the ticket a page shows, in board order, each with
// its status, and how many are done. The metadata header puts it on every
// ticket page; it shows nothing on a ticket without children.

const route = useRoute()

const here = computed(() => {
    const entry = documentAt(route.path)
    return entry?.kind === 'ticket' ? entry.label : null
})

// Board order is the columns' order, Todo first, each column top first.
const children = computed(() => {
    if (!here.value) return []
    const ordered = data.columns.flatMap(column => column.tickets)

    return childrenOf(ordered, here.value)
})

const counted = computed(() => progress(children.value))

const board = computed(() => settings.ticket
    ? `/${settings.ticket.route}/board?under=${encodeURIComponent(here.value ?? '')}`
    : null)
</script>

<template>
    <section v-if="children.length" class="ticket-children">
        <p class="ticket-children-head">
            Children, {{ counted.done }} of {{ counted.total }} done
            <a v-if="board" :href="board">on the board</a>
        </p>
        <ul class="ticket-children-list">
            <li v-for="child in children" :key="child.id">
                <span class="doc-badge" :class="'doc-badge--' + child.status.toLowerCase().replace(/\s+/g, '-')">{{ child.status }}</span>
                <span class="ticket-children-id">{{ child.id }}</span>
                <a :href="child.path">{{ child.title }}</a>
            </li>
        </ul>
    </section>
</template>

<style>
.ticket-children {
    margin: 0.5rem 0 1.5rem;
}
.ticket-children-head {
    margin: 0 0 0.4rem !important;
    font-size: 0.85rem;
    color: var(--vp-c-text-2);
}
.ticket-children-head a {
    margin-left: 0.4rem;
}
.ticket-children-list {
    list-style: none;
    padding-left: 0 !important;
    margin: 0 !important;
}
.ticket-children-list li {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    margin: 0.2rem 0;
}
.ticket-children-id {
    font-family: var(--vp-font-family-mono);
    font-size: 0.75rem;
    color: var(--vp-c-text-3);
}
</style>
