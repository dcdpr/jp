---
aside: false
prev: false
next: false
---

# RFD Priorities

The active RFD backlog in priority order — highest first, including **draft**
RFDs you want to prioritise finishing.
In a project that tracks tickets, checking an RFD's box marks it **in
development**: its tracking ticket is created if it doesn't exist yet and moved
to In Progress; unchecking moves that ticket back to Todo.
The box appears from **Accepted** onward, since that is where implementation
begins.
Every RFD above a **milestone** line targets that release.
Implemented, superseded, and abandoned RFDs are not shown here; see the [full
RFD index] for those.

<script setup>
import { data } from '../.vitepress/jp-vite/loaders/rfd-board.data.js'
import { problem } from '../.vitepress/jp-vite/theme/problem.mjs'
import RfdBoard from '../.vitepress/jp-vite/theme/RfdBoard.vue'
</script>

<RfdBoard v-if="!problem" :entries="data.entries" :priority="data.priority" />

[full RFD index]: ./
