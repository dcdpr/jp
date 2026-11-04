<script setup>
import { computed } from 'vue'

import settings from '../settings.json'
import { hasLabelTerm, labelQuery } from './label-filter.mjs'
import { decodeLabels, groupLabels, labelHref } from './label-groups.mjs'

// A document's labels, wherever the site shows them: in a page's header, a
// table row, or a board card. They read as one line per key, `domain: llm,
// mcp`. The key leads to the documents carrying any value under it, and each
// value to those carrying that label.
//
// Without `pick`, each key and value links to the default listing for `kind`
// (the ticket board or the wiki index) filtered by that term. Inside a
// listing, the caller passes `pick` to toggle the filter in place and `filter`
// to highlight active terms. Each link then points to `?label=<term>` on the
// current page so modified clicks still open the filtered view in a new tab.
//
// VitePress's router intercepts same-site `<a>` clicks at the document level
// before the element's `@click` handler runs. Setting `target="_self"` when
// `pick` is provided makes the router ignore the link so `@click` can filter
// in place.

const props = defineProps({
    // An array, or the attribute value `encodeLabels` writes: a page's header
    // is compiled from markdown and can only pass a string.
    labels: { type: [Array, String], required: true },
    // `ticket` or `wiki`, the kind of document carrying them; any other kind
    // gets no links.
    kind: { type: String, default: null },
    pick: { type: Function, default: null },
    filter: { type: Object, default: null },
})

const groups = computed(() => {
    const labels = typeof props.labels === 'string' ? decodeLabels(props.labels) : props.labels
    return groupLabels(labels).map(([key, values]) => ({
        key,
        values: values.map(value => ({ value, term: `${key}=${value}` })),
    }))
})

function href(term) {
    if (props.pick) return labelQuery(term)

    return labelHref(settings, props.kind, term)
}

// Render labels as plain text when `pick` is unset and `kind` has no
// label-filtered listing (e.g. RFDs).
const inert = computed(() => !props.pick && !href(''))

function click(event, term) {
    // Let modified clicks (Cmd, Ctrl, Shift, Alt) fall through to the browser
    // so they open in a new tab or window.
    if (!props.pick || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return
    event.preventDefault()
    props.pick(term)
}
</script>

<template>
    <div v-if="groups.length" :class="['doc-labels', { 'doc-labels--inert': inert }]">
        <span v-for="group in groups" :key="group.key" class="doc-label">
            <a
                :href="href(group.key)"
                :target="pick ? '_self' : null"
                :class="['doc-label-term', 'doc-label-key', { active: hasLabelTerm(filter, group.key) }]"
                :title="`Any ${group.key}`"
                @click="click($event, group.key)"
            >{{ group.key }}</a><template v-if="group.values.length">:</template>
            <template v-for="(part, at) in group.values" :key="part.value">
                {{ ' ' }}<a
                    :href="href(part.term)"
                    :target="pick ? '_self' : null"
                    :class="['doc-label-term', { active: hasLabelTerm(filter, part.term) }]"
                    :title="part.term"
                    @click="click($event, part.term)"
                >{{ part.value }}</a><template v-if="at < group.values.length - 1">,</template>
            </template>
        </span>
    </div>
</template>

<style>
.doc-labels {
    display: flex;
    flex-wrap: wrap;
    gap: 0.1rem 0.75rem;
    margin-top: 0.25rem;
    font-size: 0.8rem;
    line-height: 1.5;
    color: var(--vp-c-text-2);
}
.doc-label {
    white-space: nowrap;
}
/* Two classes beat VitePress's `.vp-doc a`, which colours and underlines every
   link in a page. */
.doc-labels .doc-label-term {
    border-radius: 3px;
    color: inherit;
    font-weight: inherit;
    text-decoration: none;
    cursor: pointer;
}
.doc-labels .doc-label-key {
    color: var(--vp-c-text-3);
}
.doc-labels .doc-label-term:hover {
    color: var(--vp-c-brand-1);
    text-decoration: underline;
}
.doc-labels--inert .doc-label-term,
.doc-labels--inert .doc-label-term:hover {
    color: inherit;
    text-decoration: none;
    cursor: default;
}
.doc-labels--inert .doc-label-key,
.doc-labels--inert .doc-label-key:hover {
    color: var(--vp-c-text-3);
}
.doc-labels .doc-label-term.active {
    padding: 0 0.15rem;
    background: var(--vp-c-brand-soft);
    color: var(--vp-c-brand-1);
}
</style>
