<script setup>
import { onContentUpdated, useRoute } from 'vitepress'
import { computed, onMounted, onUnmounted, ref } from 'vue'

import { isDev, postBoard } from './board.mjs'
import { documentAt } from './documents.mjs'

// Checking and unchecking a ticket's checklist items from its page, on a dev
// server that accepts edits.
//
// The items of a ticket's description render with the text they are found by
// in `data-checklist-item` (see `../tasklist.mjs`), disabled. On a ticket's
// page this enables them, and a change runs `jp ticket edit --check` or
// `--uncheck` through the dev server. The ticket file changing re-renders the
// page from it. A refused change puts the checkbox back and says why.
//
// Changes are sent one at a time, in the order they were made: each one
// rewrites the ticket file, and two at once could each write over the other.

const ENDPOINT = '/__ticket'
const SELECTOR = '.vp-doc input[data-checklist-item]'

const route = useRoute()
const ticket = computed(() => {
    const entry = documentAt(route.path)
    return entry?.kind === 'ticket' ? entry : null
})

const notice = ref(null)
let noticeTimer = null
let pending = Promise.resolve()

function showNotice(text) {
    notice.value = text
    clearTimeout(noticeTimer)
    noticeTimer = setTimeout(() => { notice.value = null }, 8000)
}

function enable() {
    if (!isDev || !ticket.value) return

    for (const box of document.querySelectorAll(SELECTOR)) box.disabled = false
}

function changed(event) {
    const box = event.target
    if (!isDev || !ticket.value || !(box instanceof HTMLInputElement) || !box.matches(SELECTOR)) {
        return
    }

    const id = ticket.value.label
    const checked = box.checked
    box.disabled = true
    pending = pending.then(() => save(box, id, checked))
}

async function save(box, id, checked) {
    const { ok, output } = await postBoard(ENDPOINT, {
        action: checked ? 'check' : 'uncheck',
        id,
        item: box.dataset.checklistItem,
    })
    box.disabled = false
    if (ok) return

    box.checked = !checked
    showNotice(output || 'The checklist item could not be saved.')
}

onContentUpdated(enable)
onMounted(() => {
    enable()
    document.addEventListener('change', changed)
})
onUnmounted(() => {
    document.removeEventListener('change', changed)
    clearTimeout(noticeTimer)
})
</script>

<template>
    <Teleport to="body">
        <div v-if="notice" class="ticket-checklist-notice" role="alert" @click="notice = null">
            {{ notice }}
        </div>
    </Teleport>
</template>

<style>
.vp-doc .task-list-item-checkbox:not(:disabled) {
    cursor: pointer;
}
.ticket-checklist-notice {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    z-index: 100;
    max-width: min(32rem, calc(100% - 2rem));
    max-height: 12rem;
    overflow: auto;
    padding: 0.6rem 0.8rem;
    border: 1px solid var(--vp-c-danger-1);
    border-radius: 6px;
    background: var(--vp-c-bg);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.2);
    color: var(--vp-c-danger-1);
    font-size: 0.8rem;
    white-space: pre-wrap;
    cursor: pointer;
}
</style>
