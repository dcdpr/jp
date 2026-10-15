---
aside: false
prev: false
next: false
---

# Ticket Board

Columns are stages of work; priority is the vertical order within a column.
Todo is therefore the work queue, read top-down, and it holds triaged and
untriaged work alike.

**Blocked** is a badge rather than a column: a blocked ticket is still at
whatever stage it reached.
Done shows only the head of the column — the [full list] has the rest.

<script setup>
import { data } from '../.vitepress/jp-vite/loaders/tickets.data.js'
import { problem } from '../.vitepress/jp-vite/theme/problem.mjs'
import TicketBoard from '../.vitepress/jp-vite/theme/TicketBoard.vue'
</script>

<TicketBoard v-if="!problem" :columns="data.columns" />

Status lives in the ticket file; the order within a column lives in the board
file, `.board.json`, next to the tickets.
A ticket the board file doesn't mention sits at the bottom of its column, so a
newly filed ticket joins Todo below whatever has already been prioritised.

[full list]: ./
