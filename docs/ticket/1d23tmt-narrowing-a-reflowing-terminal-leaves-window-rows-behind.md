# Narrowing a reflowing terminal leaves window rows behind

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-03
- **Label**: client=cli
- **Label**: package=jp_printer
- **Label**: relevant-rfd=091
- **Label**: type=bug

Narrowing a terminal that reflows its contents leaves parts of a drawn status
region on screen.
[RFD 120]'s manual check reproduced it in Ghostty, with the reasoning window at
a tenth of the terminal's height (`style.reasoning.window.rows = true`) on a
terminal 226 columns wide and 65 rows tall.
Narrowing the terminal while the assistant reasoned left a stack of partial
copies of the window above it, each narrower than the last, and the answer
printed at the end of a line of leftover fragments:

```text
I'm setting up a recurrence to track the number of ways to reach each p
splitting states by whether the last jump was a 3-step or not, then sta
I'm setting up a recurrence to track the number of ways to reach
splitting states by whether the last jump was a 3-step or not, th
I'm setting up a recurrence to track the number of ways to re
splitting states by whether the last jump was a 3-step or not
⏱ Reasoning… 8.5s                                 TT0 T0 througdropping odroppidroppingI'm reI'II'm refI'm refoI'm ref668653
```

The tool and MCP startup windows are drawn by the same code, so a row of theirs
that is wider than the new width does the same.
Making the terminal shorter than a region is a separate, known limit: the
region's top rows go to scrollback, where no erase reaches ([RFD 091]).

## Cause

A region erases and redraws itself by walking back the number of rows it drew
(`RegionStack::erase` and `RegionStack::redraw` in
`crates/jp_printer/src/region.rs`).
It reads the terminal's height on every draw and caps the walk there, but a
terminal that reflows on resize re-wraps the rows already drawn, so they cover
more rows than the walk goes back.
Each redraw while the terminal narrows leaves the part it missed behind.

A shaded row is painted by setting the background and clearing to the end of the
line (`\x1b[K`), which fills it to the right edge.
In the manual check that fill re-wrapped along with the text.
The reasoning window shades every row, and the tool window does inside a
reasoning region ([RFD 095]).

## Options

- **Shade a row only as far as its text.** Without the fill, a row re-wraps only
  once its text no longer fits.
  This works in every terminal, but stops helping below the text's width (80
  columns for prose), and the shading gets a ragged right edge, unlike inline
  reasoning.
- **Have the terminal clear the region instead of reflowing it.** Ghostty and
  kitty do that for a shell prompt marked with OSC 133, and leave the program to
  redraw it.
  The region's next redraw, within 100ms, would put it back.
  The marker also tells the terminal the shell is idle while `jp` runs: Ghostty
  then closes the tab without asking, and `jump_to_prompt` stops at the region.
  Ghostty has open reports about redraws on this path, so it needs an experiment
  before it is worth building.
- **Walk back the rows the reflow produced.** Remembering the width each row was
  drawn at gives the rows it covers after a reflow, but only for a terminal that
  reflows the way the estimate assumes.
  A terminal that truncates instead would have the walk clear output above the
  region, which is worse than leaving fragments.

## References

- Ghostty's shell integration:
  <https://ghostty.org/docs/features/shell-integration>
- How Ghostty treats a marked prompt on resize:
  <https://github.com/nushell/reedline/issues/684>
- kitty's prompt marking:
  <https://sw.kovidgoyal.net/kitty/shell-integration/#notes-for-shell-developers>
- Open Ghostty reports:
  <https://github.com/ghostty-org/ghostty/discussions/13629> and
  <https://github.com/ghostty-org/ghostty/discussions/13460>

[RFD 091]: ../rfd/091-printer-owned-status-region.md
[RFD 095]: ../rfd/095-reasoning-region-shading-across-tool-calls.md
[RFD 120]: ../rfd/120-ephemeral-reasoning-window.md
