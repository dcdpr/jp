<script setup>
import { describeLabelFilter } from './label-filter.mjs'

// The line above a listing narrowed to some labels: what it is narrowed to,
// and a way back to all of it. Shown by the ticket board and the ticket and
// wiki tables; nothing when there is no filter.

defineProps({
    filter: { type: Object, default: null },
    // What the listing holds, `Tickets` or `Pages`.
    noun: { type: String, required: true },
    clear: { type: Function, required: true },
})
</script>

<template>
    <p v-if="filter" class="doc-label-filter">
        {{ describeLabelFilter(filter, noun) }}.
        <button class="doc-label-filter-clear" @click="clear()">Show all {{ noun.toLowerCase() }}</button>
    </p>
</template>

<style>
.doc-label-filter {
    font-size: 0.85rem;
    color: var(--vp-c-text-2);
}
.doc-label-filter-clear {
    padding: 0;
    border: none;
    background: transparent;
    color: var(--vp-c-brand-1);
    font: inherit;
    cursor: pointer;
}
.doc-label-filter-clear:hover {
    text-decoration: underline;
}
</style>
