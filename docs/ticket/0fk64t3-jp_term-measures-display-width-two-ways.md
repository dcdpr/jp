# `jp_term` measures display width two ways

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-08
- **Label**: package=jp_term
- **Label**: type=task

`jp_term` now holds two independent implementations of "how wide is this text on
screen", which arrived from different directions and have never been compared.

- `jp_term::width::display_width` strips ANSI with `strip-ansi-escapes`, then
  measures with `unicode-width`.
  Its neighbours (`truncate_to_width`, `wrap_ranges`, `prefix_end_for_width`)
  walk grapheme clusters and probe for ligatures that collapse.
- `jp_term::ansi::visual_width` tokenizes with `ansi::segments`, joins the
  visible runs, and measures the result.
  `advance_column` builds on it and adds tab stops and carriage returns.

They disagree in at least one way that matters: `visual_width` counts a tab as
one column and `advance_column` exists because of it, while the `width` module
has no notion of a tab at all.
Whether they disagree on OSC 8 hyperlinks, partial escapes, or ligature collapse
is unknown — nothing tests them against each other.

## Why it is worth doing

The duplication was invisible while the two lived in different crates.
Now a reader of `jp_term` has to pick one, and the names give no help:
`display_width` and `visual_width` are the same phrase.

## Shape of a fix

Start with a test that runs both over the same inputs — plain ASCII, CJK, VS16
emoji, ZWJ sequences, tabs, OSC 8 links, escapes split mid-sequence — and
records where they differ.
That answers whether this is one function with two names or two functions with
one job each.

If they agree except on tabs, collapse to one and keep `advance_column` as the
cursor-position variant.
If they genuinely differ, the names have to say how.
