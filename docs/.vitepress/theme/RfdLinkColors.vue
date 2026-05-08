<script setup>
import { useRoute } from 'vitepress'
import { watch, nextTick, onMounted } from 'vue'
import { data } from '../../.vitepress/loaders/rfds.data.js'

const route = useRoute()

// Map RFD number -> lowercase status, e.g. "042" -> "implemented".
const statusByNum = new Map(
    data.map(r => [r.num, r.status?.toLowerCase()]).filter(([, s]) => s)
)

function applyLinkColors() {
    if (!/^\/rfd\/\d{3}-/.test(route.path)) return

    for (const a of document.querySelectorAll('.vp-doc a[href]')) {
        const raw = a.getAttribute('href') ?? ''
        // Skip pure anchors and external links — they share the current
        // page's pathname after browser resolution and would mis-tag.
        if (raw.startsWith('#') || /^[a-z][a-z0-9+.-]*:/i.test(raw)) continue
        // Use the browser-resolved absolute pathname so we match regardless
        // of whether the source href was relative (`065-foo.md`) or absolute
        // (`/rfd/065-foo`).
        const num = a.pathname?.match(/\/rfd\/(\d{3})-/)?.[1]
        if (!num) continue
        const status = statusByNum.get(num)
        if (!status) continue
        // Idempotent: skip if a status class is already present.
        if ([...a.classList].some(c => c.startsWith('rfd-link--'))) continue
        a.classList.add('rfd-link', `rfd-link--${status}`)
    }
}

onMounted(() => nextTick(applyLinkColors))
watch(() => route.path, () => nextTick(applyLinkColors))
</script>

<template>
    <!-- Renders nothing; tags RFD links via DOM manipulation. -->
</template>
