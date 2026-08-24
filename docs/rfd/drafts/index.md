---
aside: false
prev: false
next: false
---

# Draft RFDs

Drafts are proposals that haven't been assigned a permanent number yet.
They carry a temporary `DNN` id and may change substantially — or be abandoned
— before promotion.
See [RFD 001] for the full process, and the [published RFDs] for accepted and
implemented proposals.

<script setup>
import { data } from '../../.vitepress/loaders/rfd-drafts.data.js'
import DocIndex from '../../.vitepress/theme/DocIndex.vue'
</script>

<DocIndex :entries="data" :show-status="false" storage-key="rfd-drafts" />

[RFD 001]: ../001-jp-rfd-process
[published RFDs]: ../
