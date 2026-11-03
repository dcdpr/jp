<script setup>
import { useRoute } from 'vitepress'
import { computed } from 'vue'

import { ancestorsOf } from './ancestors.mjs'
import { KINDS, describe, documentAt, documentNamed } from './documents.mjs'

// Where a document sits: its kind's index, then each document above it, the
// top one first, each a link by title. The document itself is the title below,
// so it isn't repeated.
//
//     <DocCrumbs kind="wiki" always />
//     <DocCrumbs kind="ticket" />
//
// It describes the page it is on, when that is a document of `kind`, and
// follows `Parent` fields up through documents of that kind alone. A page under
// nothing shows the index alone with `always`, and nothing without it.
//
// This is where a document sits, not how the reader got there; the trail of
// pages visited is `DocBreadcrumb.vue`'s.

const props = defineProps({
    // `wiki` or `ticket`.
    kind: { type: String, required: true },
    always: { type: Boolean, default: false },
})

const route = useRoute()

const here = computed(() => {
    const entry = documentAt(route.path)
    return entry?.kind === props.kind ? entry : null
})

const root = computed(() => KINDS[props.kind] ?? null)

const ancestors = computed(() => {
    if (!here.value) return []

    return ancestorsOf(here.value.label, label => {
        const found = documentNamed(label)
        return found?.kind === props.kind ? found : null
    })
})

const shown = computed(() =>
    here.value !== null && root.value !== null && (props.always || ancestors.value.length > 0))
</script>

<template>
    <nav v-if="shown" class="doc-crumbs" aria-label="Where this page sits">
        <a :href="root.root">{{ root.name }}</a>
        <template v-for="entry in ancestors" :key="entry.path">
            <span class="doc-crumbs-sep" aria-hidden="true">›</span>
            <a :href="entry.path" :title="describe(entry)">{{ entry.title }}</a>
        </template>
    </nav>
</template>

<style>
.doc-crumbs {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.1rem 0.4rem;
    min-width: 0;
    margin-bottom: 0.5rem;
    font-size: 0.8rem;
    color: var(--vp-c-text-3);
}
.doc-crumbs a {
    color: var(--vp-c-text-2);
    text-decoration: none;
}
.doc-crumbs a:hover {
    color: var(--vp-c-brand-1);
    text-decoration: underline;
}
</style>
