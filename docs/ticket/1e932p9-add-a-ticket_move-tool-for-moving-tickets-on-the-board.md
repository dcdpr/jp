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

- `id`: target ticket ID.
- `status`: `todo`, `in progress`, or `done`.
  When omitted, the ticket stays in its current column.
- One of `before` or `after` (a ticket ID in the target column), or `position`
  (`top` or `bottom`).
- At least one of `status` and a placement is required.

`ticket_move` reuses the primitives behind `jp ticket status` and `jp ticket
rank`: `board::move_to` for a column change with an optional anchor, and
`board::rank` for reordering within a column.
Column changes go through `store::set_status` and `relate` to enforce the same
parent-child status rules as `ticket_edit`.

- `top` places the ticket before the first displayed ticket in the column;
  `bottom` places it after the last.
  `bottom` resolves that last ticket as an explicit `after` anchor because
  `move_to` without an anchor only updates the status.

- Because unranked tickets sort below ranked ones, moving a ticket to `bottom`
  records every preceding ticket in `.board.json`.

- `ticket_edit` retains its `status` parameter so callers can still update
  `status` and `parent` together in a single validated call.

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
