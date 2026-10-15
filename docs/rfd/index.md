---
aside: false
prev: false
next: false
---

# Requests for Discussion

RFDs are short design documents that describe a significant change before
implementation begins.

The active backlog in priority order lives on the [priorities] page.
Proposals that haven't been assigned a permanent number yet live on the [draft
RFDs] page.

<script setup>
import { data } from '../.vitepress/jp-vite/loaders/rfds.data.js'
import settings from '../.vitepress/jp-vite/settings.json'
import { problem } from '../.vitepress/jp-vite/theme/problem.mjs'
import DocIndex from '../.vitepress/jp-vite/theme/DocIndex.vue'
</script>

<DocIndex v-if="!problem" :entries="data" :filters="settings.rfd.categories" />

[draft RFDs]: ./drafts/
[priorities]: ./priority
