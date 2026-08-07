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
import { data } from '../.vitepress/loaders/tickets.data.js'
import TicketBoard from '../.vitepress/theme/TicketBoard.vue'
</script>

<TicketBoard :columns="data.columns" />

Status lives in the ticket file; the order within a column lives in
`docs/ticket/board.json`.
A ticket the board file doesn't mention sits at the bottom of its column, so a
newly filed ticket joins Todo below whatever has already been prioritised.

[full list]: ./
