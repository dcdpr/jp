<script setup>
import { useRoute } from 'vitepress'
import { watch, nextTick, onMounted } from 'vue'
import { data } from '../../.vitepress/loaders/rfds.data.js'

const route = useRoute()

// Map RFD number -> data entry, for status + title lookup.
const byNum = new Map(data.map(r => [r.num, r]))

function enhanceLinks() {
    // Status coloring is scoped to RFD pages where the legend makes sense.
    // Tooltips apply everywhere RFD links appear.
    const colorize = /^\/rfd\/\d{3}-/.test(route.path)

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
        const rfd = byNum.get(num)
        if (!rfd) continue

        // Don't clobber an explicit title from the markdown source.
        if (rfd.title && !a.hasAttribute('title')) {
            a.setAttribute('title', `RFD ${num}: ${rfd.title}`)
        }

        if (!colorize || !rfd.status) continue
        // Idempotent: skip if a status class is already present.
        if ([...a.classList].some(c => c.startsWith('rfd-link--'))) continue
        a.classList.add('rfd-link', `rfd-link--${rfd.status.toLowerCase()}`)
    }
}

onMounted(() => nextTick(enhanceLinks))
watch(() => route.path, () => nextTick(enhanceLinks))
</script>

<template>
    <!-- Renders nothing; tags RFD links with status classes + title tooltips. -->
</template>
