# Add a planner persona for the chat bridge

- **Status**: In Progress
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=tooling
- **Label**: type=task
- **Parent**: T-1e92tft

`.jp/config/personas/planner.toml` defines the persona that `jp serve chat` uses
for every turn.
All ticket and planning rules live in this persona rather than in the bridge
plugin.

Tools, the only permission boundary a delegated turn has:

- `'*' = { enable = false }`, then enable by name, never by glob, so a write
  tool added later stays off.
- Read: `fs_read_file`, `fs_list_files`, `fs_grep_files`, `fs_grep_user_docs`,
  `rfd_list`, `rfd_show`, `wiki_list`, `wiki_show`, `ticket_list`,
  `ticket_show`.
- Write: `ticket_create`, `ticket_comment`, `ticket_edit`, `ticket_move`, and
  the `rfd_*` and `wiki_*` writing tools.
- All at `run = "allow"`, since nobody is there to ask.
- `conversation.start_local = true`.

Instructions:

- Todo is one queue, worked from the top.
  To place a ticket for pickup around a date, estimate from the throughput
  figure in the persona (tickets closed per week, maintained by hand at first)
  and rank it after the ticket expected to be done by then.
  Count a tracking ticket or its children, not both.
- Write "Requested for pickup around YYYY-MM-DD" into the description of a
  ticket placed that way.
- Phrase every placement as a proposal pending a maintainer's review, never a
  promise.
- A ticket someone mentions that isn't on the board may be awaiting a
  maintainer's review; say so instead of filing it again.
- After an interrupted tool call, re-read the ticket or the board before writing
  again: the write may have landed.
- Answers go to a phone: short, no tables.

Try it in a terminal, in a scratch worktree, before the bridge exists: `jp
--no-interactive q --new --cfg personas/planner "..."`.
Needs `ticket_move` for the ranking half.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-07T13:01:57Z

Written as `.jp/config/personas/planner.toml` (a01ffde53).
It doesn't extend the RFD skill, because that also enables `fs_modify_file` and
the GitHub tools; the `rfd_*` tools are enabled by name instead.
The throughput is 3 tickets a week (37 Done in the 13 weeks since 2026-08-10).
Close this once it has answered and placed a ticket through the bridge.
