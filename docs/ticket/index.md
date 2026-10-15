---
aside: false
prev: false
next: false
---

# Tickets

Work items — bugs, features, chores — tracked as markdown files in the
repository.
Every ticket ever filed is listed here, newest first, including the closed ones.
The [board] shows the open work in priority order.

Write a ticket when the work is clear enough to start, and an RFD when it needs
a design first.

<script setup>
import { data } from '../.vitepress/jp-vite/loaders/tickets.data.js'
import { problem } from '../.vitepress/jp-vite/theme/problem.mjs'
import TicketIndex from '../.vitepress/jp-vite/theme/TicketIndex.vue'
</script>

<TicketIndex v-if="!problem" :tickets="data.tickets" />

[board]: ./board
