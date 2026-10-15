<script setup>
import { useRoute } from 'vitepress'
import { computed } from 'vue'
import { describe, documentAt } from './documents.mjs'

const route = useRoute()

// References cross every boundary the site has: published to draft, ticket to
// RFD, and back. They all resolve against the one document set.
const entry = computed(() => documentAt(route.path))
const references = computed(() => entry.value?.references ?? [])
const referencedBy = computed(() => entry.value?.referencedBy ?? [])

const visible = computed(() =>
    references.value.length > 0 || referencedBy.value.length > 0
)

function linkClass(ref) {
    return `doc-link--${(ref.status ?? 'unknown').toLowerCase().replace(/\s+/g, '-')}`
}
</script>

<template>
    <div v-if="visible" class="doc-references">
        <div v-if="references.length" class="doc-ref-section">
            <span class="doc-ref-label">References</span>
            <a v-for="ref in references" :key="ref.path" :href="ref.path" :title="describe(ref)" :class="['doc-ref-link', linkClass(ref)]">{{ ref.label }}</a>
        </div>
        <div v-if="referencedBy.length" class="doc-ref-section">
            <span class="doc-ref-label">Referenced by</span>
            <a v-for="ref in referencedBy" :key="ref.path" :href="ref.path" :title="describe(ref)" :class="['doc-ref-link', linkClass(ref)]">{{ ref.label }}</a>
        </div>
    </div>
</template>

<style>
.doc-references {
    display: flex;
    gap: 1.5rem;
    margin-bottom: 1rem;
    font-size: 0.85rem;
    color: var(--vp-c-text-2);
}
.doc-ref-section {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    flex-wrap: wrap;
}
.doc-ref-label {
    color: var(--vp-c-text-3);
    margin-right: 0.15rem;
}
.doc-ref-link {
    color: var(--vp-c-brand-1);
    text-decoration: none;
    padding: 0.05rem 0.35rem;
    border-radius: 4px;
    background: color-mix(in srgb, var(--vp-c-brand-1) 10%, transparent);
}
.doc-ref-link:hover {
    background: color-mix(in srgb, var(--vp-c-brand-1) 20%, transparent);
    text-decoration: none;
}
</style>
