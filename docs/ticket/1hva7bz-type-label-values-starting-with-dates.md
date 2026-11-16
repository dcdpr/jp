# Type label values, starting with dates

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-12
- **Label**: domain=tooling
- **Label**: package=jp_label
- **Label**: relevant-rfd=100
- **Label**: type=feature

A label's value is free text, so a board can carry `due=2026-12-01` but can't
ask for "tickets due before Friday": a label filter matches exact values only.
The planner persona works around it by listing every ticket with a `due` label
and comparing the dates itself.

Let a vocabulary key declare the type of its values, starting with dates:

```toml
[plugins.command.ticket.options.labels.due]
description = "When the work is wanted done."
values = []
max = 1
type = "date"
```

- **Declaration.** `type` is allowed only on an open key (`values = []`); on a
  key with listed values, or a bare key, it is a declaration error.
- **Validation.** A write is refused when a value isn't a `YYYY-MM-DD` calendar
  date, and `jp ticket check` and `jp wiki check` report existing invalid
  values.
  `Vocabulary::resolve_against` performs this check, so every consumer inherits
  it.
- **Filters.** `Selector` gains `<`, `<=`, `>`, and `>=`, as in
  `due<2026-12-01`.
  The operand must be a date, or the filter is refused; a stored value that
  isn't a date doesn't match.
  Filters stay vocabulary-free, so a ticket with a malformed value can still be
  found.
  `YYYY-MM-DD` sorts lexicographically, so comparison needs no date parsing
  beyond validation.
- **Absolute only.** Values are stored and compared as dates.
  Relative input (`1w`, `+3d`) is out of scope; it can be added later as write-
  and filter-time syntax resolved against today, without changing what is
  stored.

Labels stay optional: a board that declares no `due` key is unaffected.

`jp_label`'s vocabulary entry rejects unknown fields, so every plugin binary
that reads a vocabulary (ticket, wiki, vite, rfd tooling) is rebuilt together
with this change.

This changes the vocabulary format from RFD 100, so note it there.
The conversation query DSL in D37 may later offer a general way to filter by
typed values; this doesn't wait for it.

- [x] `type = "date"` on an open vocabulary key, refused on any other key
- [x] Writes refuse a value that isn't a date; `check` reports existing ones
- [x] Comparison operators on `Selector`, used by `jp ticket list` and
      `ticket_list`
- [x] Tool definitions describe the comparison operators for date-typed keys
- [x] `due` declared with `type = "date"`; its description and the planner
      prompt drop relative values
- [x] Planner persona uses the filter for "what's due"
- [x] RFD 100 documents `type` and the comparison operators
