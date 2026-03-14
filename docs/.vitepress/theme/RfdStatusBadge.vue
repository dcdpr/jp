<script setup>
import { useRoute } from 'vitepress'
import { watch, nextTick, onMounted } from 'vue'

const route = useRoute()

const statusClasses = {
    implemented: 'rfd-badge--implemented',
    accepted: 'rfd-badge--accepted',
    discussion: 'rfd-badge--discussion',
    draft: 'rfd-badge--draft',
    superseded: 'rfd-badge--superseded',
    abandoned: 'rfd-badge--abandoned',
}

function applyBadge() {
    if (!/^\/rfd\/(\d{3}|D\d{2})-/.test(route.path)) return

    const items = document.querySelectorAll('.vp-doc li')
    for (const li of items) {
        const strong = li.querySelector('strong')
        if (!strong || strong.textContent.trim() !== 'Status') continue

        // Already transformed on a previous pass.
        if (li.querySelector('.rfd-badge')) return

        // The li innerHTML looks like: <strong>Status</strong>: Implemented
        // Extract the value after the colon.
        const text = li.textContent.replace(/^Status\s*:\s*/, '').trim()
        if (!text) return

        const key = text.toLowerCase()
        const cls = statusClasses[key] ?? ''

        const badge = document.createElement('span')
        badge.className = `rfd-badge ${cls}`
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

