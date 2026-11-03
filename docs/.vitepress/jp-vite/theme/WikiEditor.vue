<script setup>
import { useRoute } from 'vitepress'
import { computed, nextTick, onMounted, ref, watch } from 'vue'

import DocCrumbs from './DocCrumbs.vue'
import { isDev } from './board.mjs'
import { documentAt } from './documents.mjs'

// The line above a wiki page's title: where the page sits, and on a dev server
// that accepts edits, a button to edit it.
//
// Editing a wiki page happens from the page itself, on a dev server that
// accepts edits.
//
// Each section heading gets an "edit" button, and the page an "Edit page"
// button. Either opens the text as written, embeds as the images that name
// them, in a panel over the page. Saving goes through `jp wiki edit`, which
// checks the page before keeping it: an edit that breaks a link or the format
// is refused, and the panel shows why. A section's text opens with its
// heading, so changing that line retitles the section.
//
// Headings an embed brought in carry no anchor, so they get no button: they
// are edited on the page they come from.

const ENDPOINT = '/__wiki'

const route = useRoute()
const page = computed(() => {
    const entry = documentAt(route.path)
    return entry?.kind === 'wiki' ? entry : null
})
const active = computed(() => isDev && page.value !== null)

// `null` when closed; otherwise what is being edited: `{ section, title, text }`,
// `section` `null` for the whole page.
const editing = ref(null)
const busy = ref(false)
const notice = ref(null)

async function open(section, title) {
    notice.value = null
    busy.value = true
    editing.value = { section, title, text: '' }

    const query = new URLSearchParams({ id: page.value.label })
    if (section) query.set('section', section)
    try {
        const res = await fetch(`${ENDPOINT}?${query}`)
        const detail = await res.json()
        if (!detail?.ok) {
            notice.value = detail?.output ?? 'could not read the page'
            return
        }
        editing.value.text = section ? detail.text ?? '' : detail.body ?? ''
    } catch (err) {
        notice.value = String(err.message ?? err)
    } finally {
        busy.value = false
    }
}

async function save() {
    busy.value = true
    notice.value = null
    try {
        const payload = { action: 'edit', id: page.value.label, body: editing.value.text }
        if (editing.value.section) payload.section = editing.value.section

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

function close() {
    editing.value = null
    notice.value = null
}

// Add an edit button to each section heading of the page, once.
function decorate() {
    if (!active.value) return

    for (const heading of document.querySelectorAll('.vp-doc :is(h2, h3, h4)[id]')) {
        if (heading.closest('.jp-embed') || heading.querySelector('.wiki-edit-section')) continue

        const button = document.createElement('button')
        // `ignore-header` keeps the button out of the outline's heading text.
        button.className = 'wiki-edit-section ignore-header'
        button.type = 'button'
        button.textContent = 'edit'
        button.title = 'Edit this section'
        button.addEventListener('click', () => {
            const title = heading.textContent.replace(/edit$/, '').replace(/​/g, '').trim()
            open(heading.id, title)
        })
        heading.appendChild(button)
    }
}

onMounted(() => nextTick(decorate))
watch(() => route.path, () => {
    close()
    nextTick(decorate)
})
</script>

<template>
    <div v-if="page" class="wiki-editor-bar">
        <DocCrumbs kind="wiki" always />
        <button v-if="active" class="wiki-editor-open" type="button" :disabled="busy" @click="open(null, page.title)">
            Edit page
        </button>
    </div>

    <Teleport to="body">
        <div v-if="editing" class="wiki-editor-backdrop" @click.self="close()">
            <form class="wiki-editor" @submit.prevent="save()">
                <div class="wiki-editor-head">
                    <strong>{{ editing.section ? `Edit section: ${editing.title}` : `Edit ${page?.label}` }}</strong>
                    <span v-if="busy" class="wiki-editor-busy">Working…</span>
                </div>
                <textarea
                    v-model="editing.text"
                    class="wiki-editor-text"
                    spellcheck="true"
                    :disabled="busy"
                ></textarea>
                <p v-if="notice" class="wiki-editor-error">{{ notice }}</p>
                <div class="wiki-editor-actions">
                    <button class="wiki-editor-save" type="submit" :disabled="busy">Save</button>
                    <button type="button" :disabled="busy" @click="close()">Cancel</button>
                </div>
            </form>
        </div>
    </Teleport>
</template>

<style>
.wiki-editor-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    margin-bottom: 0.5rem;
}
.wiki-editor-open {
    flex-shrink: 0;
}
/* The bar keeps the space below; the crumbs share its line with the button. */
.wiki-editor-bar .doc-crumbs {
    margin-bottom: 0;
}
.wiki-editor-open,
.wiki-edit-section {
    padding: 0.1rem 0.5rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 4px;
    background: transparent;
    color: var(--vp-c-text-2);
    font-size: 0.75rem;
    font-weight: 400;
    cursor: pointer;
}
.wiki-edit-section {
    margin-left: 0.6rem;
    vertical-align: middle;
    opacity: 0;
    transition: opacity 0.15s;
}
.vp-doc :is(h2, h3, h4):hover .wiki-edit-section,
.wiki-edit-section:focus-visible {
    opacity: 1;
}
/* No hover on a touch screen, so the buttons always show there. */
@media (hover: none) {
    .wiki-edit-section {
        opacity: 1;
    }
}
.wiki-editor-open:hover,
.wiki-edit-section:hover {
    border-color: var(--vp-c-brand-1);
    color: var(--vp-c-text-1);
}
.wiki-editor-backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1rem;
    background: rgba(0, 0, 0, 0.45);
}
.wiki-editor {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    width: min(64rem, 100%);
    height: min(48rem, 100%);
    padding: 1rem;
    border-radius: 8px;
    background: var(--vp-c-bg);
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.3);
}
.wiki-editor-head {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
}
.wiki-editor-busy {
    color: var(--vp-c-text-3);
    font-size: 0.85rem;
}
.wiki-editor-text {
    flex: 1;
    padding: 0.5rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 4px;
    background: var(--vp-c-bg-soft);
    color: var(--vp-c-text-1);
    font-family: var(--vp-font-family-mono);
    font-size: 0.85rem;
    line-height: 1.5;
    resize: none;
}
.wiki-editor-error {
    margin: 0;
    max-height: 8rem;
    overflow: auto;
    white-space: pre-wrap;
    font-size: 0.8rem;
    color: var(--vp-c-danger-1);
}
.wiki-editor-actions {
    display: flex;
    gap: 0.5rem;
}
.wiki-editor-actions button {
    padding: 0.25rem 0.9rem;
    border: 1px solid var(--vp-c-divider);
    border-radius: 4px;
    background: var(--vp-c-bg-soft);
    color: var(--vp-c-text-1);
    cursor: pointer;
}
.wiki-editor-actions .wiki-editor-save {
    border-color: var(--vp-c-brand-1);
    background: var(--vp-c-brand-1);
    color: var(--vp-c-white);
}
</style>
