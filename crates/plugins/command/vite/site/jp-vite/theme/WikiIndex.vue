<script setup>
import { nextTick, ref } from 'vue'

import { data } from '../loaders/wiki.data.js'
import DocIndex from './DocIndex.vue'
import { isDev } from './board.mjs'
import { problem } from './problem.mjs'
import { topPages } from './wiki-tree.mjs'

// The wiki index, which a page renders with `<WikiIndex />`: the shared
// document table, filtered by the top page each page sits under, plus add,
// edit, and archive on the dev server.
//
// Every write goes through `jp wiki`, so the browser never has to know what a
// page file looks like. A page is archived rather than deleted, so links to it
// still resolve. The page reloads afterwards: the list comes from a build-time
// data loader, and a reload is the honest way to pick up what changed on disk.

// The titles of the top pages, which the table filters by: each page's `area`.
const AREAS = topPages(data).map(page => page.title)
// The pages a page can sit under, in reading order, each indented as deep as
// it sits.
const PARENTS = data
    .filter(page => page.position !== null)
    .sort((a, b) => a.position - b.position)

// A select can't style its options, so depth is told with non-breaking spaces.
function indent(depth) {
    return String.fromCharCode(0xa0).repeat(depth * 2)
}
const STATUSES = ['Reference', 'Snapshot', 'Archived']
// A page starts as a Reference or a Snapshot, and is archived later.
const NEW_STATUSES = STATUSES.filter(status => status !== 'Archived')
const ENDPOINT = '/__wiki'

// The fields an edit compares against what the page held when the form opened.
const FIELDS = ['title', 'summary', 'status', 'parent', 'body']

// `null` when closed, otherwise the page being edited or a blank one to add.
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
    form.value = {
        action: 'add',
        title: '',
        summary: '',
        status: 'Reference',
        // Empty for a top page.
        parent: '',
        labels: '',
        body: '',
    }
    reveal()
}

// The labels field holds them as the index shows them, comma-separated.
function splitLabels(text) {
    return text.split(',').map(label => label.trim()).filter(Boolean)
}

// A page's own text isn't in the index data, so it's read back from the page
// file before the form is shown.
async function openEdit(page) {
    notice.value = null
    busy.value = true
    form.value = null

    try {
        const res = await fetch(`${ENDPOINT}?id=${encodeURIComponent(page.id)}`)
        const detail = await res.json()
        if (!detail?.ok) {
            notice.value = detail?.output ?? 'could not read the page'
            return
        }

        const fields = {
            title: detail.title ?? '',
            summary: detail.summary ?? '',
            status: detail.status ?? 'Reference',
            parent: detail.parent ?? '',
            body: detail.body ?? '',
            labels: (detail.labels ?? []).join(', '),
        }
        form.value = { action: 'edit', id: page.id, ...fields, before: fields }
        reveal()
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
    const current = form.value
    if (!current.title.trim()) {
        notice.value = 'A page needs a title.'
        return
    }

    if (current.action === 'add') {
        if (!current.summary.trim()) {
            notice.value = 'A page needs a one-sentence summary.'
            return
        }
        await post({
            action: 'add',
            title: current.title,
            status: current.status,
            summary: current.summary,
            parent: current.parent,
            body: current.body,
            labels: splitLabels(current.labels),
        })
        return
    }

    // Only what changed is sent, so an edit doesn't rewrite what it didn't
    // touch, and a label nobody touched stays as written.
    const payload = { action: 'edit', id: current.id }
    for (const field of FIELDS) {
        if (current[field] !== current.before[field]) payload[field] = current[field]
    }
    if (current.labels !== current.before.labels) {
        payload.labels = splitLabels(current.labels)
        payload.labelsBefore = splitLabels(current.before.labels)
    }

    await post(payload)
}

async function archive(page) {
    const ok = window.confirm(
        `Archive ${page.id}: ${page.title}?\n\n`
        + `The page is kept so links to it still resolve, and leaves the sidebar. `
        + `A page with active child pages can't be archived.`,
    )
    if (!ok) return

    await post({ action: 'archive', id: page.id })
}
</script>

<template>
<template v-if="!problem">
<!-- Above the table: on a long list, a form appended at the bottom is a scroll
     away from the row whose Edit button opened it. -->
<div v-if="isDev" ref="panel" class="wiki-admin">
    <div class="wiki-admin-row">
        <button class="doc-filter" :disabled="busy" @click="openAdd()">+ New page</button>
        <span v-if="notice" class="wiki-admin-error">{{ notice }}</span>
    </div>

    <form v-if="form" class="wiki-admin-form" @submit.prevent="submit()">
        <div class="wiki-admin-row">
            <strong>{{ form.action === 'add' ? 'New page' : 'Edit ' + form.id }}</strong>
            <select v-model="form.parent" class="doc-search" aria-label="Parent">
                <option value="">A top page</option>
                <option
                    v-for="page in PARENTS.filter(page => page.id !== form.id)"
                    :key="page.id"
                    :value="page.id"
                >{{ indent(page.depth) }}{{ page.title }}</option>
            </select>
            <select v-model="form.status" class="doc-search" aria-label="Status">
                <option
                    v-for="status in (form.action === 'add' ? NEW_STATUSES : STATUSES)"
                    :key="status"
                    :value="status"
                >{{ status }}</option>
            </select>
        </div>
        <input v-model="form.title" class="doc-search" placeholder="Title" />
        <input v-model="form.summary" class="doc-search" placeholder="One-sentence summary" />
        <input
            v-model="form.labels"
            class="doc-search"
            placeholder="Labels, as key=value, separated by commas"
        />
        <textarea
            v-model="form.body"
            class="doc-search wiki-admin-body"
            rows="12"
            placeholder="The page's text, as markdown"
        ></textarea>
        <div class="wiki-admin-row">
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
    :entries="data"
    storage-key="wiki"
    id-label="Page"
    id-field="num"
    :filters="AREAS"
    filter-field="area"
    label-kind="wiki"
    label-noun="Pages"
>
    <!-- Icons, not labels: two words per row costs more width than a phone has
         to spare. `title` and `aria-label` carry the meaning. -->
    <template v-if="isDev" #actions="{ entry }">
        <button
            class="wiki-icon"
            :disabled="busy"
            title="Edit"
            aria-label="Edit"
            @click="openEdit(entry)"
        >✎</button>
        <button
            v-if="entry.status !== 'Archived'"
            class="wiki-icon"
            :disabled="busy"
            title="Archive"
            aria-label="Archive"
            @click="archive(entry)"
        >⌫</button>
    </template>
</DocIndex>
</template>
</template>

<style>
.wiki-admin {
    margin-top: 2rem;
}
.wiki-admin-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
}
.wiki-admin-form {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin: 1rem 0;
    padding: 0.75rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 6px;
    background: var(--vp-c-bg-soft);
}
.wiki-admin-body {
    font-family: var(--vp-font-family-mono);
    resize: vertical;
}
.wiki-admin-error {
    font-size: 0.8rem;
    color: var(--vp-c-danger-1);
}
/* Big enough to hit with a thumb, narrow enough not to squeeze the title. */
.wiki-icon {
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
.wiki-icon + .wiki-icon {
    margin-left: 0.35rem;
}
.wiki-icon:hover {
    color: var(--vp-c-text-1);
    border-color: var(--vp-c-text-3);
}
.wiki-icon:disabled {
    opacity: 0.5;
    cursor: default;
}
</style>
