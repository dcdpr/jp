<script setup>
import { useRoute } from 'vitepress'
import { ref, watch, computed, onMounted } from 'vue'

const route = useRoute()
const trail = ref([])

const STORAGE_KEY = 'rfd-trail'

function rfdNum(path) {
    return path.match(/\/rfd\/(\d{3})-/)?.[1] ?? null
}

function rfdSlug(path) {
    return path.match(/\/rfd\/((\d{3})-.+?)(?:\.html)?$/)?.[1] ?? null
}

function saveTrail() {
    try { sessionStorage.setItem(STORAGE_KEY, JSON.stringify(trail.value)) } catch {}
}

function loadTrail() {
    try {
        const raw = sessionStorage.getItem(STORAGE_KEY)
        if (raw) trail.value = JSON.parse(raw)
    } catch {}
}

function onNavigate(path) {
    if (path === '/rfd/' || path === '/rfd') {
        trail.value = []
        saveTrail()
        return
    }

    const num = rfdNum(path)
    const slug = rfdSlug(path)
    if (!num || !slug) return

    const idx = trail.value.findIndex(e => e.num === num)
    if (idx !== -1) {
        trail.value = trail.value.slice(0, idx + 1)
    } else {
        trail.value = [...trail.value, { num, slug }]
    }
    saveTrail()
}

const visible = computed(() => /^\/rfd\/\d{3}-/.test(route.path))

onMounted(() => {
    loadTrail()
    onNavigate(route.path)
})

watch(() => route.path, onNavigate)
</script>

<template>
    <nav v-if="visible" class="rfd-breadcrumb">
        <a href="/rfd/">RFDs</a>
        <template v-for="(entry, i) in trail" :key="entry.num">
            <span class="rfd-breadcrumb-sep">/</span>
            <a v-if="i < trail.length - 1" :href="'/rfd/' + entry.slug">{{ entry.num }}</a>
            <span v-else class="rfd-breadcrumb-current">{{ entry.num }}</span>
        </template>
    </nav>
</template>

<style>
.rfd-breadcrumb {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    margin-bottom: 1rem;
    color: var(--vp-c-text-3);
}
.rfd-breadcrumb a {
    color: var(--vp-c-brand-1);
    text-decoration: none;
}
.rfd-breadcrumb a:hover {
    text-decoration: underline;
}
.rfd-breadcrumb-sep {
    color: var(--vp-c-text-3);
}
.rfd-breadcrumb-current {
    color: var(--vp-c-text-2);
}
</style>
