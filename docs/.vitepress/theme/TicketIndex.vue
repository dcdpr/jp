<script setup>
import { nextTick, ref } from 'vue'

import DocIndex from './DocIndex.vue'
import { isDev } from './board.mjs'

// The ticket index: the shared document table, plus add / edit / delete on the
// dev server.
//
// Every write goes through `jp ticket`, so the browser never has to know what a
// ticket file looks like. The page reloads afterwards: the list comes from a
// build-time data loader, and a reload is the honest way to pick up what changed
// on disk.

const props = defineProps({
    tickets: { type: Array, required: true },
})

const KINDS = ['bug', 'feature', 'chore']
const STATUSES = ['Todo', 'In Progress', 'Done']
const ENDPOINT = '/__ticket'

// `null` when closed, otherwise the ticket being edited or a blank one to add.
const form = ref(null)
const busy = ref(false)
const notice = ref(null)
const panel = ref(null)

// The form sits above the table, so opening it from a row far down the list
// would otherwise put it off-screen.
async function reveal() {
    await nextTick()
    panel.value?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

function openAdd() {
    notice.value = null
    form.value = { action: 'add', kind: 'bug', title: '', body: '', status: 'Todo' }
    reveal()
}

// The description isn't in the index data (the loader strips it), so it's read
// back from the ticket file before the textarea is shown.
async function openEdit(ticket) {
    notice.value = null
    busy.value = true
    form.value = {
        action: 'edit',
        id: ticket.id,
        kind: (ticket.kind ?? 'bug').toLowerCase(),
        title: ticket.title ?? '',
        body: '',
        status: ticket.status ?? 'Todo',
    }
    reveal()

    try {
        const res = await fetch(`${ENDPOINT}?id=${encodeURIComponent(ticket.id)}`)
        const detail = await res.json()
        if (detail?.ok && form.value?.id === ticket.id) {
            form.value.body = detail.ticket?.description ?? ''
        } else if (!detail?.ok) {
            notice.value = detail?.output ?? 'could not read the ticket'
        }
    } catch (err) {
        notice.value = String(err.message ?? err)
    } finally {
        busy.value = false
    }
}

async function post(payload) {
    busy.value = true
    try {
        const res = await fetch(ENDPOINT, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(payload),
        })
        const result = await res.json().catch(() => ({ ok: false, output: 'no response' }))
        if (!result.ok) {
            notice.value = result.output
            return
        }
        // Straight back to disk-truth rather than patching the page's copy.
        window.location.reload()
    } catch (err) {
        notice.value = String(err.message ?? err)
    } finally {
        busy.value = false
    }
}

async function submit() {
    if (!form.value.title.trim()) {
        notice.value = 'A ticket needs a title.'
        return
    }
    await post(form.value)
}

async function remove(ticket) {
    const ok = window.confirm(
        `Delete ${ticket.id} — ${ticket.title}?\n\n`
        + `This removes the file. The number stays retired.`,
    )
    if (!ok) return

    await post({ action: 'delete', id: ticket.id })
}
</script>

<template>
<!-- Above the table: on a long list, a form appended at the bottom is a scroll
     away from the row whose Edit button opened it. -->
<div v-if="isDev" ref="panel" class="ticket-admin">
    <div class="ticket-admin-row">
        <button class="doc-filter" :disabled="busy" @click="openAdd()">+ New ticket</button>
        <span v-if="notice" class="ticket-admin-error">{{ notice }}</span>
    </div>

    <form v-if="form" class="ticket-admin-form" @submit.prevent="submit()">
        <div class="ticket-admin-row">
            <strong>{{ form.action === 'add' ? 'New ticket' : 'Edit ' + form.id }}</strong>
            <select v-model="form.kind" class="doc-search">
                <option v-for="kind in KINDS" :key="kind" :value="kind">{{ kind }}</option>
            </select>
            <select v-if="form.action === 'edit'" v-model="form.status" class="doc-search">
                <option v-for="status in STATUSES" :key="status" :value="status">{{ status }}</option>
            </select>
        </div>
        <input v-model="form.title" class="doc-search" placeholder="Title" />
        <textarea
            v-model="form.body"
            class="doc-search ticket-admin-body"
            rows="6"
            placeholder="Description"
        ></textarea>
        <div class="ticket-admin-row">
            <button class="doc-filter active" type="submit" :disabled="busy">
                {{ busy ? 'Saving…' : 'Save' }}
            </button>
            <button class="doc-filter" type="button" :disabled="busy" @click="form = null">
                Cancel
            </button>
        </div>
    </form>
</div>

<DocIndex
    :entries="props.tickets"
    storage-key="ticket"
    id-label="Ticket"
    id-field="num"
    :filters="KINDS"
    filter-field="kind"
    :show-summary="false"
    :show-authors="true"
>
    <!-- Icons, not labels: two words per row costs more width than a phone has
         to spare. `title` and `aria-label` carry the meaning. -->
    <template v-if="isDev" #actions="{ entry }">
        <button
            class="ticket-icon"
            :disabled="busy"
            title="Edit"
            aria-label="Edit"
            @click="openEdit(entry)"
        >✎</button>
        <button
            class="ticket-icon ticket-icon--danger"
            :disabled="busy"
            title="Delete"
            aria-label="Delete"
            @click="remove(entry)"
        >✕</button>
    </template>
</DocIndex>
</template>

<style>
.ticket-admin {
    margin-top: 2rem;
}
.ticket-admin-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
}
.ticket-admin-form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin: 1rem 0;
    padding: 0.75rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 6px;
    background: var(--vp-c-bg-soft);
}
.ticket-admin-body {
    font-family: var(--vp-font-family-mono);
    resize: vertical;
}
.ticket-admin-error {
    font-size: 0.8rem;
    color: var(--vp-c-danger-1);
}
/* Big enough to hit with a thumb, narrow enough not to squeeze the title. */
.ticket-icon {
    width: 2rem;
    height: 2rem;
    padding: 0;
    font-size: 1rem;
    line-height: 1;
    color: var(--vp-c-text-2);
    background: transparent;
    border: 1px solid var(--vp-c-divider);
    border-radius: 6px;
    cursor: pointer;
}
.ticket-icon + .ticket-icon {
    margin-left: 0.35rem;
}
.ticket-icon:hover {
    color: var(--vp-c-text-1);
    border-color: var(--vp-c-text-3);
}
.ticket-icon--danger:hover {
    color: var(--vp-c-danger-1);
    border-color: var(--vp-c-danger-1);
}
.ticket-icon:disabled {
    opacity: 0.5;
    cursor: default;
}
</style>
