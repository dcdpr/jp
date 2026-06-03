<script setup>
import { useRoute } from 'vitepress'
import { computed } from 'vue'
import { data as published } from '../../.vitepress/loaders/rfds.data.js'
import { data as drafts } from '../../.vitepress/loaders/rfd-drafts.data.js'

const route = useRoute()

// References can cross the published/draft boundary (a draft may reference a
// published RFD), so resolve targets against the combined set.
const data = [...published, ...drafts]

const rfd = computed(() => {
    const num = route.path.match(/\/rfd\/(?:drafts\/)?(\d{3}|D\d{2})-/)?.[1]
    if (!num) return null
    return data.find(r => r.num === num) ?? null
})

const references = computed(() => {
    if (!rfd.value?.references?.length) return []
    return rfd.value.references
        .map(num => data.find(r => r.num === num))
        .filter(Boolean)
})

const referencedBy = computed(() => {
    if (!rfd.value?.referencedBy?.length) return []
    return rfd.value.referencedBy
        .map(num => data.find(r => r.num === num))
        .filter(Boolean)
})

const visible = computed(() =>
    rfd.value && (references.value.length > 0 || referencedBy.value.length > 0)
)
</script>

<template>
    <div v-if="visible" class="rfd-references">
        <div v-if="references.length" class="rfd-ref-section">
            <span class="rfd-ref-label">References</span>
            <a v-for="ref in references" :key="ref.num" :href="ref.path" :title="`RFD ${ref.num}: ${ref.title}`" :class="['rfd-ref-link', 'rfd-link--' + (ref.status?.toLowerCase() ?? 'unknown')]">{{ ref.num }}</a>
        </div>
        <div v-if="referencedBy.length" class="rfd-ref-section">
            <span class="rfd-ref-label">Referenced by</span>
            <a v-for="ref in referencedBy" :key="ref.num" :href="ref.path" :title="`RFD ${ref.num}: ${ref.title}`" :class="['rfd-ref-link', 'rfd-link--' + (ref.status?.toLowerCase() ?? 'unknown')]">{{ ref.num }}</a>
        </div>
    </div>
</template>

<style>
.rfd-references {
    display: flex;
    gap: 1.5rem;
    margin-bottom: 1rem;
    font-size: 0.85rem;
    color: var(--vp-c-text-2);
}
.rfd-ref-section {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    flex-wrap: wrap;
}
.rfd-ref-label {
    color: var(--vp-c-text-3);
    margin-right: 0.15rem;
}
.rfd-ref-link {
    color: var(--vp-c-brand-1);
    text-decoration: none;
    padding: 0.05rem 0.35rem;
    border-radius: 4px;
    background: color-mix(in srgb, var(--vp-c-brand-1) 10%, transparent);
}
.rfd-ref-link:hover {
    background: color-mix(in srgb, var(--vp-c-brand-1) 20%, transparent);
    text-decoration: none;
}
</style>
