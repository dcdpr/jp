<script setup>
import { computed } from 'vue'

import { data } from '../loaders/wiki.data.js'
import settings from '../settings.json'
import { ancestorsOf } from './wiki-tree.mjs'

// Where a wiki page sits: the wiki, then each page above it, the top one first,
// each a link. The page itself is the title below, so it isn't repeated.
//
//     Wiki › Glossary › Ubiquitous Language

const props = defineProps({
    // The page, `W-0a1b2c3`.
    id: { type: String, required: true },
})

const root = `/${settings.wiki?.route ?? 'wiki'}/`
const ancestors = computed(() => ancestorsOf(data, props.id))
</script>

<template>
    <nav class="wiki-crumbs" aria-label="Where this page sits">
        <a :href="root">Wiki</a>
        <template v-for="page in ancestors" :key="page.id">
            <span class="wiki-crumbs-sep" aria-hidden="true">›</span>
            <a :href="page.path">{{ page.title }}</a>
        </template>
    </nav>
</template>

<style>
.wiki-crumbs {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.1rem 0.4rem;
    min-width: 0;
    font-size: 0.8rem;
    color: var(--vp-c-text-3);
}
.wiki-crumbs a {
    color: var(--vp-c-text-2);
    text-decoration: none;
}
.wiki-crumbs a:hover {
    color: var(--vp-c-brand-1);
    text-decoration: underline;
}
</style>
