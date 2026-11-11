# Ask before the planner's write tools run

- **Status**: Todo
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
