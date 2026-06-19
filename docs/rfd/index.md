---
aside: false
prev: false
next: false
---

# Requests for Discussion

RFDs are short design documents that describe a significant change before
implementation begins.
See [RFD 001] for the full process.

- **Design** — feature proposals and architectural changes
- **Decision** — recording a specific choice: a technology, convention, or
  standard
- **Guide** — how-tos and reference material for contributors
- **Process** — how the project operates: workflows, policies, values

The active backlog in priority order lives on the [priorities] page. Proposals
that haven't been assigned a permanent number yet live on the [draft RFDs] page.

<script setup>
import { data } from '../.vitepress/loaders/rfds.data.js'
import RfdIndex from '../.vitepress/theme/RfdIndex.vue'
</script>

<RfdIndex :entries="data" />

[RFD 001]: ./001-jp-rfd-process
[priorities]: ./priority
[draft RFDs]: ./drafts/
