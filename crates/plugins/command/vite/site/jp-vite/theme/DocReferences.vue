<script setup>
import { useRoute } from 'vitepress'
import { computed } from 'vue'

import { documentAt } from './documents.mjs'
import { FOLD_ABOVE, groupReferences, referenceLabel } from './reference-groups.mjs'

// What a document references and what references it, below its text: each
// direction a list grouped by kind, every document named by its title. A long
// list starts folded, so it never stands between a reader and the page.
//
// References cross every boundary the site has: published to draft, ticket to
// RFD, wiki page to both, and back. They all resolve against the one document
// set.

const route = useRoute()
const entry = computed(() => documentAt(route.path))

const sections = computed(() => [
    { name: 'References', entries: entry.value?.references ?? [] },
    { name: 'Referenced by', entries: entry.value?.referencedBy ?? [] },
]
    .filter(section => section.entries.length > 0)
    .map(section => ({
        ...section,
        count: section.entries.length,
        groups: groupReferences(section.entries),
    })))

function linkClass(ref) {
    return `doc-link--${(ref.status ?? 'unknown').toLowerCase().replace(/\s+/g, '-')}`
}
</script>

<template>
    <div v-if="sections.length" class="doc-references">
        <details
            v-for="section in sections"
            :key="section.name"
            class="doc-ref-section"
            :open="section.count <= FOLD_ABOVE"
        >
            <summary class="doc-ref-summary">
                {{ section.name }} <span class="doc-ref-count">{{ section.count }}</span>
            </summary>
            <div v-for="group in section.groups" :key="group.kind" class="doc-ref-group">
                <span class="doc-ref-kind">{{ group.name }}</span>
                <ul class="doc-ref-list">
                    <li v-for="ref in group.entries" :key="ref.path">
                        <a :href="ref.path" :class="['doc-ref-link', linkClass(ref)]">{{ ref.title || referenceLabel(ref) }}</a>
                        <span v-if="ref.title" class="doc-ref-id">{{ referenceLabel(ref) }}</span>
                    </li>
                </ul>
            </div>
        </details>
    </div>
</template>

<style>
.doc-references {
    margin-top: 3rem;
    padding-top: 1rem;
    border-top: 1px solid var(--vp-c-divider);
    font-size: 0.875rem;
}
.doc-ref-section + .doc-ref-section {
    margin-top: 0.75rem;
}
.doc-ref-summary {
    cursor: pointer;
    font-weight: 600;
    color: var(--vp-c-text-2);
}
.doc-ref-count {
    font-weight: 400;
    color: var(--vp-c-text-3);
}
.doc-ref-group {
    margin: 0.5rem 0 0 1rem;
}
.doc-ref-kind {
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--vp-c-text-3);
}
.doc-ref-list {
    list-style: none;
    margin: 0.15rem 0 0;
    padding: 0;
}
.doc-ref-list li {
    margin: 0.1rem 0;
}
.doc-ref-link {
    text-decoration: none;
}
.doc-ref-link:hover {
    text-decoration: underline;
}
.doc-ref-id {
    margin-left: 0.4rem;
    font-family: var(--vp-font-family-mono);
    font-size: 0.75rem;
    color: var(--vp-c-text-3);
}
</style>
