<script setup>
import { data } from '../loaders/rfds.data.js'
import settings from '../settings.json'
import DocIndex from './DocIndex.vue'
import { problem } from './problem.mjs'

// The published RFDs as a table, which a page renders with `<RfdIndex />`.
</script>

<template>
<DocIndex v-if="!problem" :entries="data" :filters="settings.rfd?.categories ?? []" />
</template>
