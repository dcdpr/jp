<script setup>
import { useRoute } from 'vitepress'
import { computed } from 'vue'

import settings from '../settings.json'
import { problem } from './problem.mjs'

// The problem `jp vite serve` found before starting the site, on every page the
// RFD and ticket loaders feed. The index and board pages show nothing else; a
// document still shows its own text below it, which it reads from its file.

const route = useRoute()

const prefixes = ['rfd', 'ticket']
    .map(kind => settings[kind]?.route)
    .filter(Boolean)
    .map(path => `/${path}/`)

const visible = computed(() =>
    problem !== null && prefixes.some(prefix => `${route.path}/`.startsWith(prefix)))
</script>

<template>
    <div v-if="visible" class="site-problem">
        <p class="site-problem-title">This site may misread its documents, so it doesn't show them.</p>
        <pre class="site-problem-text">{{ problem }}</pre>
    </div>
</template>

<style>
.site-problem {
    margin-bottom: 1.5rem;
    padding: 0.75rem 1rem;
    border: 1px solid var(--vp-c-danger-1);
    border-radius: 6px;
    background: var(--vp-c-danger-soft);
}
.site-problem-title {
    margin: 0 0 0.5rem !important;
    font-weight: 600;
    color: var(--vp-c-danger-1);
}
.site-problem-text {
    margin: 0 !important;
    white-space: pre-wrap;
    font-size: 0.85rem;
    line-height: 1.5;
}
</style>
