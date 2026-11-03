# Type reasoning into the window at the typewriter's pace

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-03
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_printer
- **Label**: relevant-rfd=120
- **Label**: type=enhancement

Reasoning in the window (`style.reasoning.window`) appears a whole row at a
time, where inline reasoning types out a character at a time, and the rows
popping in read worse than the typewriter.

Both paths hold back the line being written until it wraps
(`handle_paragraph_chunk` in `crates/jp_cli/src/render/chat.rs`).
Inline, the finished line goes to the printer as a typewriter task, written a
character at a time `style.typewriter.text_delay` apart (3ms by default, and
`code_delay` for code).
In the window, `ChatRenderer::emit` pushes it through `LineSink::push` as one
row, and the region draws it whole on its next redraw.
[RFD 120] chose this in point 7 of its Rendering path ("No typewriter delay").

## Proposal

Reveal each window row at the typewriter's pace, so the window types the way
inline reasoning does.

- `LineSink` takes a row together with its per-character delay, and the renderer
  passes the delay `emit` already receives.
  The tool and MCP startup windows keep pushing rows without one, and draw them
  whole.
- The worker keeps a reveal position per region.
  Rows before it are drawn whole, the row at it is cut to the columns revealed
  so far with `truncate_row`, which keeps escape sequences whole, and rows after
  it are not drawn yet.
- The position follows elapsed time, so a late tick catches up instead of
  slowing the reveal.
  While a row is being revealed, `RegionStack::tick_after` asks for a frame
  every 16ms or so rather than the status row's `interval`; `MIN_INTERVAL`
  allows down to 10ms.
- Releasing the window drops the rows not revealed yet, so the answer does not
  wait for the preview.
  Inline, it queues behind the typing.
- No new configuration: `style.typewriter` paces the window as it paces the
  transcript, and `text_delay = "0s"` draws rows whole again.

The change updates point 7 of RFD 120's Rendering path to match.

At the rate Anthropic streamed Opus's reasoning in RFD 120's manual check (about
80 characters a second), a row completes about once a second and then types out
in a quarter of a second at the default delay, which is the rhythm inline
reasoning has.

## Open questions

- **How far the reveal may fall behind.** Reasoning that streams faster than the
  reveal, 333 characters a second at the default delay, leaves it further behind
  for as long as it streams.
  `style.typewriter.max_latency` bounds the lag inline when it is set, and could
  bound the window the same way.
  Its default of `0s` sets no bound; capping the backlog at the window's height,
  and revealing anything older at once, keeps the window on current reasoning.
- **Repaint cost.** The window repaints once per finished row, and RFD 120's
  manual check saw no flicker at that rate.
  A reveal repaints every frame.
  Repainting only the row being revealed and the status row keeps frames small.
  Synchronized output (`CSI ? 2026 h` and `CSI ? 2026 l`) would make each frame
  atomic, and Ghostty supports it, but Ghostty has open reports of it corrupting
  the screen around incremental cursor-positioned updates, so it needs testing
  there before it is relied on.

## Alternative

Show the line being written as it arrives: the renderer pushes the held-back
line as an open row that the next chunk replaces, committed when it wraps.
The window then shows reasoning a line sooner, but at the provider's pace rather
than the typewriter's: in RFD 120's manual check, Anthropic sent reasoning about
ten characters at a time, eight times a second.
The open row also shows what holding the line back hides, such as its last word
jumping to the next row once it stops fitting.
It composes with the proposal, which would reveal the open row as it grows.

## References

- Ghostty on synchronized output:
  <https://ghostty.org/docs/help/synchronized-output>
- The open report: <https://github.com/ghostty-org/ghostty/discussions/12062>

[RFD 120]: ../rfd/120-ephemeral-reasoning-window.md
