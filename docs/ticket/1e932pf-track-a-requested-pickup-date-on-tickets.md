# Track a requested pickup date on tickets

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=enhancement

When the `planner` persona schedules a ticket for pickup around a target date
(T-1e92tft), it records that date only in the ticket description and ranks the
ticket in Todo.
As other tickets are inserted above it, its position drifts, and nothing in `jp
ticket` can query which tickets are due soon or flag tickets that have slipped
past their requested date.

Two options to evaluate:

- An open-valued label key in `plugins.command.ticket.options.labels`, which
  requires no code changes and supports filtering.
- A dedicated metadata field rendered on the board.

Either approach turns a requested pickup date into a visible board field, so we
should decide whether we want the board to surface date commitments before
implementing one.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T19:28:16Z

Done as an open-valued `due` label, at most one per ticket: a date,
`due=2026-11-25`, or a duration after the ticket's `Date`, `due=1w`.
The planner persona always writes a date, since that is what a `ticket_list`
label filter can find.
No board field or code change; a field stays possible if dates need sorting.
