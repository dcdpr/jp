# Ask before the planner's write tools run

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=plugins
- **Label**: type=task
- **Parent**: T-1e92tft

Once T-1hetcnt lands, set the write tools in `.jp/config/personas/planner.toml`
to `run = "ask"`: `ticket_create`, `ticket_comment`, `ticket_edit`,
`ticket_move`, and the `rfd_*` and `wiki_*` tools that write.
Leave the read tools at `run = "allow"`, so questions about the work don't each
need an approval.

Until T-1hetcnt lands, `ask` behaves like `allow` in a turn the bridge starts,
so this change does nothing before it.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-12T12:54:32Z

Dropped: the planner keeps approving its own tool calls for now.
Asking before a write is part of T-1hetcnt, once its design lands (T-1hva7by).
