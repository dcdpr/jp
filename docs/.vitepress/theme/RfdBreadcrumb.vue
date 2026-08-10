<script setup>
import { useRoute } from 'vitepress'
import { ref, watch, computed, onMounted } from 'vue'
import { KINDS, describe, documentAt, originAt } from './documents.mjs'

const route = useRoute()
const trail = ref([])
const origin = ref(null)

// One trail across both kinds, so following an RFD link out of a ticket reads
// `Tickets / T0001 / 042`. Entries store the path; everything else is looked up
// on render.
const STORAGE_KEY = 'doc-trail'

function saveTrail() {
    const state = { origin: origin.value, trail: trail.value }
    try { sessionStorage.setItem(STORAGE_KEY, JSON.stringify(state)) } catch {}
}

function loadTrail() {
    try {
        const raw = sessionStorage.getItem(STORAGE_KEY)
        if (!raw) return
        const state = JSON.parse(raw)
        origin.value = state.origin ?? null
        trail.value = state.trail ?? []
    } catch {}
}

function onNavigate(path) {
    // Landing on an index or a board starts a fresh trail, rooted there.
    const landed = originAt(path)
    if (landed) {
        origin.value = landed
        trail.value = []
        saveTrail()
        return
    }

    const entry = documentAt(path)
    if (!entry) return

    // Revisiting something already in the trail truncates back to it, rather
    // than growing a loop.
    const index = trail.value.findIndex(e => e.path === entry.path)
    trail.value = index === -1
        ? [...trail.value, { path: entry.path }]
        : trail.value.slice(0, index + 1)
    saveTrail()
}

// The page the trail started from, or — for a cold link straight into a
// document — that kind's index.
const root = computed(() => {
    if (origin.value) return origin.value

    const first = trail.value[0] && documentAt(trail.value[0].path)

    return KINDS[first?.kind ?? documentAt(route.path)?.kind ?? 'rfd']
})

const crumbs = computed(() =>
    trail.value.map(e => documentAt(e.path)).filter(Boolean))

const visible = computed(() => documentAt(route.path) !== null)

onMounted(() => {
    loadTrail()
    onNavigate(route.path)
})

watch(() => route.path, onNavigate)
</script>

<template>
    <nav v-if="visible" class="doc-breadcrumb">
        <a :href="root.path ?? root.root">{{ root.name }}</a>
        <template v-for="(entry, i) in crumbs" :key="entry.path">
            <span class="doc-breadcrumb-sep">/</span>
            <a v-if="i < crumbs.length - 1" :href="entry.path" :title="describe(entry)">{{ entry.label }}</a>
            <span v-else class="doc-breadcrumb-current">{{ entry.label }}</span>
        </template>
    </nav>
</template>

<style>
.doc-breadcrumb {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    margin-bottom: 1rem;
    color: var(--vp-c-text-3);
}
.doc-breadcrumb a {
    color: var(--vp-c-brand-1);
    text-decoration: none;
}
.doc-breadcrumb a:hover {
    text-decoration: underline;
}
.doc-breadcrumb-sep {
    color: var(--vp-c-text-3);
}
.doc-breadcrumb-current {
    color: var(--vp-c-text-2);
}
</style>
