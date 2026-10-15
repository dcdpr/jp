---
aside: false
prev: false
next: false
---

# Draft RFDs

Drafts are proposals that haven't been assigned a permanent number yet.
They carry a temporary `DNN` id and may change substantially — or be abandoned
— before promotion.
See the [published RFDs] for accepted and implemented proposals.

<script setup>
import { data } from '../../.vitepress/jp-vite/loaders/rfd-drafts.data.js'
import settings from '../../.vitepress/jp-vite/settings.json'
import { problem } from '../../.vitepress/jp-vite/theme/problem.mjs'
import DocIndex from '../../.vitepress/jp-vite/theme/DocIndex.vue'
</script>

<DocIndex v-if="!problem" :entries="data" :filters="settings.rfd.categories" :show-status="false" storage-key="rfd-drafts" />

[published RFDs]: ../
