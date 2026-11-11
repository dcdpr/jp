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

Tool configuration (the sole permission boundary for non-interactive bridge
turns):

- Disable all tools by default (`'*' = { enable = false }`) and enable allowed
  tools explicitly by name rather than by glob, so newly added write tools stay
  disabled.
- Read: `fs_read_file`, `fs_list_files`, `fs_grep_files`, `fs_grep_user_docs`,
  `rfd_list`, `rfd_show`, `wiki_list`, `wiki_show`, `ticket_list`,
  `ticket_show`.
- Write: `ticket_create`, `ticket_comment`, `ticket_edit`, `ticket_move`, and
  the `rfd_*` and `wiki_*` writing tools.
- Set `run = "allow"` on every enabled tool because turns run unattended.
- `conversation.start_local = true`.

Instructions:

- Todo is one queue, worked from the top.
  To place a ticket for pickup around a date, estimate from the throughput
  figure in the persona (tickets closed per week, maintained by hand at first)
  and rank it after the ticket expected to be done by then.
  Count a tracking ticket or its children, not both.
- Record `"Requested for pickup around YYYY-MM-DD"` in the description of newly
  filed scheduled tickets (or as a comment on existing ones).
- Phrase every placement as a proposal pending a maintainer's review, never a
  promise.
- If a sender references a ticket ID that is not on the board, note that a
  maintainer may have already taken it into an unmerged review branch rather
  than filing a duplicate.
- After an interrupted tool call, re-read the ticket or board before retrying in
  case the write already succeeded.
- Format responses for mobile chat clients: keep answers brief and avoid
  Markdown tables or headings.

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
