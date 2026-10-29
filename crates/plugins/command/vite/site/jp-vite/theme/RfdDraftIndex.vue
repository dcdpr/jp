<script setup>
import { data } from '../loaders/rfd-drafts.data.js'
import settings from '../settings.json'
import DocIndex from './DocIndex.vue'
import { problem } from './problem.mjs'

// The draft RFDs as a table, which a page renders with `<RfdDraftIndex />`.
// Drafts all share one status, so the table leaves it out.
</script>

<template>
<DocIndex
    v-if="!problem"
    :entries="data"
    :filters="settings.rfd?.categories ?? []"
    :show-status="false"
    storage-key="rfd-drafts"
/>
</template>
