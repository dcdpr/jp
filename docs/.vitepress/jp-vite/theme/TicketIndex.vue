<script setup>
import { nextTick, ref } from 'vue'

import { data } from '../loaders/tickets.data.js'
import DocIndex from './DocIndex.vue'
import { isDev } from './board.mjs'
import { problem } from './problem.mjs'

// The ticket index, which a page renders with `<TicketIndex />`: the shared
// document table, plus add / edit / delete on the dev server.
//
// Every write goes through `jp ticket`, so the browser never has to know what a
// ticket file looks like. The page reloads afterwards: the list comes from a
// build-time data loader, and a reload is the honest way to pick up what changed
// on disk.

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
    form.value = { action: 'add', title: '', body: '', labels: '', status: 'Todo' }
    reveal()
}

// The labels field holds them as the index shows them, comma-separated.
function splitLabels(text) {
    return text.split(',').map(label => label.trim()).filter(Boolean)
}

// The description isn't in the index data (the loader strips it), so it's read
// back from the ticket file before the textarea is shown.
async function openEdit(ticket) {
    notice.value = null
    busy.value = true
    const labels = (ticket.labels ?? []).join(', ')
    form.value = {
        action: 'edit',
        id: ticket.id,
        title: ticket.title ?? '',
        body: '',
        labels,
        labelsBefore: labels,
        status: ticket.status ?? 'Todo',
    }
    reveal()

    try {
        const res = await fetch(`${ENDPOINT}?id=${encodeURIComponent(ticket.id)}`)
        const detail = await res.json()
        if (detail?.ok && form.value?.id === ticket.id) {
            // `jp -F json ticket show` flattens the ticket into the top level,
            // next to its `id` and `path`.
            form.value.body = detail.description ?? ''
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
    const { labelsBefore, ...payload } = form.value
    if (!payload.title.trim()) {
        notice.value = 'A ticket needs a title.'
        return
    }

    // Labels replace the whole set, so they are sent only when they changed: a
    // title fix shouldn't fail on a ticket whose labels the board would refuse
    // as they stand.
    if (payload.action === 'edit' && payload.labels === labelsBefore) {
        delete payload.labels
    } else {
        payload.labels = splitLabels(payload.labels)
    }

    await post(payload)
}

async function remove(ticket) {
    const ok = window.confirm(
        `Delete ${ticket.id} — ${ticket.title}?\n\n`
        + `This removes the file. A later ticket in the same time bucket can draw the id again.`,
    )
    if (!ok) return

    await post({ action: 'delete', id: ticket.id })
}
</script>

<template>
<template v-if="!problem">
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
            <select v-if="form.action === 'edit'" v-model="form.status" class="doc-search">
                <option v-for="status in STATUSES" :key="status" :value="status">{{ status }}</option>
            </select>
        </div>
        <input v-model="form.title" class="doc-search" placeholder="Title" />
        <input
            v-model="form.labels"
            class="doc-search"
            placeholder="Labels, as key=value, separated by commas"
        />
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
    :entries="data.tickets"
    storage-key="ticket"
    id-label="Ticket"
    id-field="num"
    :filters="[]"
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
