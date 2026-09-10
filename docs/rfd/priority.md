---
aside: false
prev: false
next: false
---

# RFD Priorities

The active RFD backlog in priority order — highest first, including **draft**
RFDs you want to prioritise finishing.
Checking an RFD's box marks it **in development**: its tracking [ticket] is
created if it doesn't exist yet and moved to In Progress; unchecking moves that
ticket back to Todo.
The box appears from **Accepted** onward, since that is where implementation
begins.
Every RFD above a **milestone** line targets that release.
Implemented, superseded, and abandoned RFDs are not shown here; see the [full
RFD index] for those.

See [RFD 001] for the process behind these documents.

<script setup>
import { data } from '../.vitepress/loaders/rfd-board.data.js'
import RfdBoard from '../.vitepress/theme/RfdBoard.vue'
</script>

<RfdBoard :entries="data.entries" :priority="data.priority" />

[RFD 001]: ./001-jp-rfd-process
[full RFD index]: ./
[ticket]: ../ticket/board
