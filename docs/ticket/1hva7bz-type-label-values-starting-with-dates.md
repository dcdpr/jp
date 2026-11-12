# Type label values, starting with dates

- **Status**: Todo
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

- **Validation.** A write is refused when a value isn't a `YYYY-MM-DD` date, and
  `jp ticket check` reports existing ones that aren't.
- **Relative input.** A write may give a relative value such as `1w` or `3d`.
  It is stored as the date it resolves to, counted from the ticket's own `Date`,
  so a stored value never goes stale.
- **Filters.** `ticket_list` and `jp ticket list` can filter a date-typed key by
  range, for example `due<2026-12-01` or `before` / `after` arguments.

Labels stay optional: a board that declares no `due` key is unaffected.

This changes the vocabulary format from RFD 100, so note it there.
The conversation query DSL in D37 may later offer a general way to filter by
typed values; this doesn't wait for it.

- [ ] `type = "date"` on a vocabulary key, with validation on write and in
      `check`
- [ ] Relative values resolved against the ticket's date on write
- [ ] Range filters for date-typed keys in `ticket_list` and `jp ticket list`
- [ ] Planner persona uses the filter for "what's due"
