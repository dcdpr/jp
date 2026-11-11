# Add a ticket_move tool for moving tickets on the board

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: relevant-rfd=116
- **Label**: type=feature
- **Parent**: T-1e92tft

The assistant can file and edit tickets but can't rank them: `ticket_edit` has
no position, and only the CLI's `rank` and `status --before/--after` reach the
board order.
Editing `docs/ticket/.board.json` by hand skips the checks the board code makes.

`ticket_move` moves a ticket to another column, reorders it within a column, or
both:

- `id`: the ticket.
- `status`: `todo`, `in progress`, or `done`; omitted, it stays in its column.
- One of `before` or `after` (a ticket id in the target column), or `position`
  (`top` or `bottom`).
- At least one of `status` and a placement is required.

Built on what `jp ticket status` and `jp ticket rank` already use:
`board::move_to` for a column change with an optional anchor, `board::rank`
within a column.
A column change goes through `store::set_status` and `relate`, so it keeps the
parent rules `ticket_edit`'s `status` keeps.

- `top` is before the first ticket of the column as it is shown, `bottom` after
  the last.
  `bottom` needs that last ticket as an explicit anchor: `move_to` without one
  only changes the status.

- Unranked tickets sort below ranked ones, so moving a ticket to the bottom
  writes every ticket above it into `.board.json`.
  Expected, not a bug.

- `status` stays on `ticket_edit` too: it changes status and parent in one
  checked write, which this tool doesn't.

- [x] `move` handled in `crates/plugins/command/ticket/src/tool.rs`, with its
      capabilities (including creating `.board.json` on a board that has none)

- [x] Moving to Done warns about unchecked checklist items, as `move_ticket` in
      `lib.rs` does

- [x] `tools/move.toml`, registered in `definitions.rs`, synced into
      `.jp/mcp/tools/ticket/`

- [x] Listed in `.jp/config/skill/ticket.toml`, with its `[conversation.tools]`
      entry

- [x] Tests assert the exact outcome text for: a column change, before/after,
      top, bottom, an anchor in another column, and a refused parent move
