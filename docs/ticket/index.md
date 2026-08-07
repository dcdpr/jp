---
aside: false
prev: false
next: false
---

# Tickets

Work items — bugs, features, chores — tracked as markdown files in the
repository, alongside the RFDs.
Every ticket ever filed is listed here, newest first, including the closed ones.
The [board] shows the open work in priority order.

Write a ticket when the work is clear enough to start, and an [RFD] when it needs
a design first.
See [RFD 100] for the format and the process.

<script setup>
import { data } from '../.vitepress/loaders/tickets.data.js'
import DocIndex from '../.vitepress/theme/DocIndex.vue'
</script>

<DocIndex
    :entries="data.tickets"
    storage-key="ticket"
    id-label="Ticket"
    id-field="num"
    :filters="['bug', 'feature', 'chore']"
    filter-field="kind"
    :show-summary="false"
    :show-authors="true"
/>

[board]: ./board
[RFD]: ../rfd/
[RFD 100]: ../rfd/100-in-repo-ticket-tracking
