<script setup>
import { useRoute } from 'vitepress'
import { watch, nextTick, onMounted } from 'vue'
import { describe, documentAt } from './documents.mjs'

const route = useRoute()

function enhanceLinks() {
    // Status colouring is scoped to document pages, where the legend makes
    // sense. Tooltips apply everywhere a document link appears.
    const colorize = documentAt(route.path) !== null

    for (const a of document.querySelectorAll('.vp-doc a[href]')) {
        const raw = a.getAttribute('href') ?? ''
        // Skip pure anchors and external links — they share the current
        // page's pathname after browser resolution and would mis-tag.
        if (raw.startsWith('#') || /^[a-z][a-z0-9+.-]*:/i.test(raw)) continue
        // Use the browser-resolved absolute pathname so we match regardless
        // of whether the source href was relative (`065-foo.md`) or absolute
        // (`/rfd/065-foo`).
        const entry = documentAt(a.pathname)
        if (!entry) continue

        // Don't clobber an explicit title from the markdown source.
        if (entry.title && !a.hasAttribute('title')) {
            a.setAttribute('title', describe(entry))
        }

        if (!colorize || !entry.status) continue
        // Idempotent: skip if a status class is already present.
        if ([...a.classList].some(c => c.startsWith('doc-link--'))) continue
        const status = entry.status.toLowerCase().replace(/\s+/g, '-')
        a.classList.add('doc-link', `doc-link--${status}`)
    }
}

onMounted(() => nextTick(enhanceLinks))
watch(() => route.path, () => nextTick(enhanceLinks))
</script>

<template>
    <!-- Renders nothing; tags document links with status classes + tooltips. -->
</template>
