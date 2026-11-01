<script setup>
import { useRoute } from 'vitepress'
import { computed } from 'vue'

import { data } from '../loaders/wiki.data.js'
import { documentAt } from './documents.mjs'
import { childrenOf, topPages } from './wiki-tree.mjs'

// The wiki pages one level down, each with its summary, in reading order, so a
// page introducing a subject leads on to the pages that go deeper:
//
// - `<WikiChildren />` on a wiki page lists the pages under it.
// - `<WikiChildren of="W-0a1b2c3" />` lists the pages under another page.
// - `<WikiChildren />` anywhere else, such as the wiki's front page, lists the
//   top pages.
//
// The list follows the pages' `Parent` and the wiki's order file, so it stays
// current as pages are added or moved. A wiki page with no text of its own
// shows this list without asking for it.

const props = defineProps({
    of: { type: String, default: null },
})

const route = useRoute()

const here = computed(() => {
    const entry = documentAt(route.path)
    return entry?.kind === 'wiki' ? entry.label : null
})

const pages = computed(() => {
    const parent = props.of ?? here.value
    return parent ? childrenOf(data, parent) : topPages(data)
})
</script>

<template>
    <ul v-if="pages.length" class="wiki-children-list">
        <li v-for="page in pages" :key="page.id">
            <a :href="page.path">{{ page.title }}</a>
            <span v-if="page.summary" class="wiki-children-summary">{{ page.summary }}</span>
        </li>
    </ul>
</template>

<style>
.wiki-children-list {
    list-style: none;
    padding-left: 0 !important;
}
.wiki-children-list li {
    margin: 0.5rem 0;
}
.wiki-children-summary {
    display: block;
    font-size: 0.875rem;
    color: var(--vp-c-text-2);
}
</style>
