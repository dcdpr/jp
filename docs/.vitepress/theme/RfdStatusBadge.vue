<script setup>
import { useRoute } from 'vitepress'
import { watch, nextTick, onMounted } from 'vue'
import { documentAt } from './documents.mjs'

const route = useRoute()

function applyBadge() {
    // Every document page: RFDs, drafts, and tickets all open with a `Status`
    // line, and all draw from the same badge palette.
    if (!documentAt(route.path)) return

    const items = document.querySelectorAll('.vp-doc li')
    for (const li of items) {
        const strong = li.querySelector('strong')
        if (!strong || strong.textContent.trim() !== 'Status') continue

        // Already transformed on a previous pass.
        if (li.querySelector('.doc-badge')) return

        // The li innerHTML looks like: <strong>Status</strong>: Implemented
        // Extract the value after the colon.
        const text = li.textContent.replace(/^Status\s*:\s*/, '').trim()
        if (!text) return

        const key = text.toLowerCase().replace(/\s+/g, '-')

        const badge = document.createElement('span')
        badge.className = `doc-badge doc-badge--${key}`
        badge.textContent = text

        // Replace li contents: keep <strong> and colon, swap the rest for the badge.
        li.textContent = ''
        li.appendChild(strong)
        li.append(': ')
        li.appendChild(badge)
        return
    }
}

onMounted(() => nextTick(applyBadge))
watch(() => route.path, () => nextTick(applyBadge))
</script>

<template>
    <!-- Renders nothing; applies badge via DOM manipulation. -->
</template>

