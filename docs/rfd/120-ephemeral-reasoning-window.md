# RFD 120: Ephemeral Reasoning Window

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-07
- **Extends**: [RFD 091]
- **Summary**: Streams rendered reasoning into a bounded status-region window
  that is erased once the assistant stops reasoning.

## Summary

Add `style.reasoning.window`, which routes rendered reasoning into a
fixed-height window at the bottom of the terminal while the assistant reasons,
and erases it when reasoning ends.
The window is built on the printer-owned status region ([RFD 091]) and composes
with the existing `style.reasoning.display` modes instead of replacing them.

## Motivation

`style.reasoning.display` offers two kinds of reasoning display, and neither
fits a common way of working:

- **Persistent** (`full`, `truncate`): reasoning is printed into the transcript.
  A long reasoning block pushes the question, the tool calls, and the start of
  the answer off screen, and leaves hundreds of lines in scrollback that are
  read once, while they stream, and never again.
- **Placeholder** (`static`, `progress`, `timer`, `hidden`): the user sees that
  the assistant is reasoning, but not what it is reasoning about.
  They cannot tell whether it understood the question, and cannot interrupt
  early when it heads in the wrong direction.

What is missing is the middle ground: watch the reasoning while it happens, then
have it go away.

JP already does exactly this for other output.
While MCP servers start (`style.mcp_startup.stderr_rows`) and while tools run
(`style.tool_call.stderr_rows`), the last few lines of the child's stderr scroll
by in a bounded window above a status row, and the whole block is erased before
the next persistent write.
Reasoning is the one stream of transient text that cannot use it.

Doing nothing leaves users choosing between a noisy transcript and no visibility
at all.

## Design

### What the user sees

```toml
[style.reasoning]
display = "full"

[style.reasoning.window]
rows = 8
```

While the assistant reasons, the bottom of the terminal shows the most recent
rows of rendered reasoning above a status row:

```txt
── jp (anthropic/claude-opus-4-8) ─────────────────────────────────────

 The loader already handles the empty case, so what is left is whether
 `extends` paths are resolved before or after glob expansion. Looking
 at `load_config_file_with_extends`, the glob is expanded relative to
 the declaring file, which means
⏱ Reasoning… 4.1s
```

The rows are the same output `display = "full"` produces: markdown styling,
wrapping, code highlighting, and the `style.reasoning.background` shading.
As new rows complete, the oldest scroll out of the window.

When anything persistent is about to render (message text, a tool call, the end
of the turn, a stream retry), the window and its status row are erased.
No reasoning is left on screen.
Resizing the terminal while the window is drawn is the exception (see
[Drawbacks](#drawbacks)): narrowing a terminal that reflows its contents can
leave parts of the window on screen, and making any terminal shorter than the
window pushes the window's top rows into scrollback, where the erase cannot
reach them.
An interrupt menu erases the window when it opens, like any prompt.
Reasoning that resumes after the user picks *continue*, or after a tool call,
opens a fresh, empty window.

The reasoning is still recorded in the conversation stream.
`jp conversation print --style full` replays it as before; the window governs
live display only.

### Configuration

One new section, `style.reasoning.window`, with two keys:

```toml
[style.reasoning.window]
# Rows of reasoning shown in the window.
# false (default): no window; reasoning renders inline as `display` says.
# true: a tenth of the terminal height.
# N: exactly N rows, still bounded by the terminal height.
rows = 8

# Show a status row below the window with the elapsed time
# ("⏱ Reasoning… 4.1s"). false shows the rows of reasoning alone.
# Defaults to true.
timer = true
```

`rows` takes the same values as the existing `stderr_rows` keys and shares its
type.

The window is orthogonal to *what* `display` renders, not to whether it renders
anything:

| `display`                     | `window.rows = false` | `window.rows` set, window renders                |
| ----------------------------- | --------------------- | ------------------------------------------------ |
| `full`                        | inline, persistent    | all reasoning, in the window                     |
| `truncate` (`<number>`)       | first N chars inline  | first N chars in the window, then the status row |
| `summary`                     | summary inline        | summary in the window                            |
| `hidden`                      | nothing               | nothing; the window has no effect                |
| `static`, `progress`, `timer` | placeholder           | placeholder; the window has no effect            |

The last two rows exist because `display` mixes two things: which reasoning
content to show, and what to show in place of content.
The placeholder modes have no content to put in a window.

A combination where the window has no effect is accepted silently, not reported
as an error.
Config is layered: a user-global `window.rows = 8` combined with a
per-conversation `--cfg style.reasoning.display=hidden` is a reasonable thing to
write, and refusing it would make one of the two layers unusable.
The doc comments on `display` and `window.rows` state the rule.

### When the window cannot render

A status region renders only on an interactive stderr, with a pretty output
format, and with no tracing layer writing to stderr ([RFD 091]).
Outside those conditions the claim yields an inert handle.
A region can also render with no room for window rows: when the terminal's
height is unknown, or when `rows = true` resolves to zero on a terminal shorter
than ten rows, such as a small split pane ([RFD 091]).

In either case the renderer behaves as if no window were configured: reasoning
renders inline as `display` says, with the same spacing, shading, and channel as
today.
The window is a placement preference, not a decision about whether reasoning is
shown, so an environment that cannot place it does not hide it.

This includes the common redirect: `jp query > answer.md` resolves `--format
auto` to `text`, which disables status regions ([RFD 091]), so reasoning is
written to `answer.md` exactly as it is without a window.
This describes the current shared `--format` resolution; if [RFD 104] lands,
whether the window renders and how its rows are styled follow stderr's
capabilities instead.

The renderer decides placement once per reasoning block, before it formats the
block's first row, because the two paths format differently (see wrap width and
background below).
It asks the printer how many window rows `rows` resolves to at the terminal's
current size through a new read-only accessor, `Printer::window_rows`, which
returns zero when regions cannot render or when no row fits.
A non-zero answer places the block in the window; zero places it inline.
The placement holds until the block ends: a resize that removes every window row
mid-block leaves the status row alone and does not move the rest of the block
inline.

### Rendering path

Apart from `Printer::window_rows`, no printer changes are needed.
The chat renderer already produces exactly what a `LineSink` accepts.

```txt
ChatResponse::Reasoning
  → ChatRenderer::render_reasoning
  → markdown buffer + formatter          (unchanged: wraps, styles,
                                          holds the in-progress line)
  → emit reasoning output
      ├─ no window:  printer.print(…)     (today)
      └─ window:     split on '\n' → LineSink::push(row)
```

`handle_paragraph_chunk` re-renders the streaming paragraph and emits only
committed visual lines, holding the in-progress line back until it wraps or the
paragraph ends.
`print_block` and `print_code` emit whole rendered blocks.
Rows reaching the printer are therefore styled, and prose rows are wrapped,
which is the shape the window was built for.
Code lines are the exception: `Formatter::render_code_line` highlights a line
but does not wrap it, so a code line wider than the terminal is cut at the
window's edge.

The renderer changes:

1. **One emission seam.** Reasoning reaches the printer from `print_block`,
   `handle_paragraph_chunk`, `print_code`, and `emit_pending_separator`.
   Each of them goes through one method that either prints, or splits on `\n`
   and pushes the rows into the window.
   A trailing segment with no `\n` is held and prefixed to the next emission, so
   a partial row never enters the window; a segment still held when the window
   is released is discarded with it.
   Message content is never routed to the window.

2. **Claim, lifetime, and release.** The window is a `StatusRegion` claimed on
   the first reasoning chunk of a block and held in the field that holds the
   timer region today (`reasoning_timer`, renamed to cover both uses).
   A second field records the block's placement, window or inline.
   Routing reads the placement, never `is_active()`, so releasing the window
   does not turn the rest of its block into inline output.

   The window stays claimed across consecutive reasoning chunks:
   `flush_on_transition` releases it only when the next content is not
   reasoning.
   The timer keeps its unconditional release there, which it depends on because
   the timer path never records a content kind.

   The other sites that release the timer today release the window too: `flush`,
   `flush_for_continuation`, the tool-call boundary, `skip_tool_call`, and
   `reset`.
   Each keeps its order, release then drain the markdown buffer.
   Reasoning drained after the release belongs to a block placed in the window,
   so it is discarded rather than printed: it would have been erased with the
   window a moment later, and the event builder has already recorded it.

   The claim keeps the timer's 300ms delay: reasoning shorter than that never
   flashes a window, and longer reasoning appears with its rows already
   buffered.

3. **Live flow only.** The window is claimed only when the renderer's flow is
   `RenderFlow::Live`.
   Replay renders reasoning as `display` says, whatever `window` says.

4. **Wrap width.** Windowed rows land on stderr, so prose wraps at
   `style.markdown.wrap_width` capped by the chrome channel's width
   (`printer.chrome_columns()`), not stdout's (`printer.output_width()`).
   With stdout redirected, stdout has no width and the cap would otherwise not
   apply.
   Inline reasoning keeps today's stdout-based width.

5. **Background.** On a terminal, `style.reasoning.background` reaches the
   screen as a `\x1b[K` line fill emitted by the formatter (`line_fill` in
   `jp_term::background`), and the region's line filter drops every non-SGR
   escape, `\x1b[K` included.
   Windowed reasoning is formatted with the reasoning background but
   `BackgroundFill::Content`, which emits no fill.
   The formatter keeps the background behind the text and restores it after
   inline elements that set their own, such as an inline code span that closes
   with `\x1b[49m`.
   The rest of each row is filled by the region, whose background is set through
   `StatusRegion::background()`: the printer worker shades every row it draws,
   the status row included.

6. **Bookkeeping follows `timer`.** Windowed reasoning is ephemeral chrome.
   It leaves nothing in the transcript, so it must not count as reasoning that
   is on screen:

   - `reasoning_supplies_separation` returns `false`.
   - `set_response_kind(Reasoning)` is not called, so `extend_across_tool_calls`
     ([RFD 095]) does not shade the next tool call.
     There is no persistent reasoning around it to extend.
   - No separator is deferred past the window.
     Blank lines between reasoning blocks become empty rows inside the window;
     the separator owed at the end of a reasoning block is dropped.

7. **No typewriter delay.** `LineSink::push` bypasses the typewriter.
   The delay paces persistent writes; the window is a preview.

### Configuration types

`StderrRows` (`jp_config::style::stderr_rows`) is renamed to `WindowRows` in a
preparatory change, since its name and doc comments describe child processes.
The existing `stderr_rows` keys and their serialized values do not change.

A new `ReasoningWindowConfig` holds `rows: WindowRows` (default `Off`) and
`timer: bool` (default `true`), nested in `ReasoningConfig` as `window`, with
the usual `AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and
`ToPartial` implementations.
The renderer maps `rows` onto the printer's `OutputLines` with the existing
`output_lines` helper in `render/tool.rs`.

## Drawbacks

- **Two keys interact.** Some `display` × `window` combinations do nothing.
  Users scanning the `display` values will not discover `window`, so the doc
  comments carry the weight.
- **Reasoning cannot be scrolled back to during the session.** Once erased, the
  only way to read it is `jp conversation print --style full`.
  A future `keep` setting (see [Non-Goals](#non-goals)) mitigates this.
- **The same config renders differently by environment.** With a window
  configured, reasoning is ephemeral in an interactive terminal and persistent
  under `-v`, `--log`, a non-terminal stderr, or a non-pretty format, because
  each of those disables status regions.
  A user who set a window to keep scrollback clean gets the reasoning back in
  scrollback whenever they debug with `-v`.
- **Narrowing the terminal can leave parts of the window behind.** Rows are
  wrapped once, when rendered, and a redraw after the terminal narrows truncates
  them to the new width ([RFD 091]'s every-row-fits invariant).
  A terminal that reflows its contents on resize, as Ghostty does, also re-wraps
  the rows already on screen, shading included.
  The window then spans more rows than the region walks back to erase, so each
  redraw while the terminal narrows leaves part of the last frame on screen.
  The manual check in phase 3 reproduced this, and the tool and MCP startup
  windows share it.
  Inline reasoning is hard-wrapped the same way, but nothing erases it, so a
  reflow only re-wraps it.
  A terminal shrunk below the window's height can also leave the window's top
  rows in scrollback, because the erase cannot reach rows the viewport has lost
  ([RFD 091]).
- **Long code lines are cut.** Inline, a code line wider than the terminal
  soft-wraps; in the window it is truncated at the edge, because a soft-wrapped
  row would break the region's row count ([RFD 091]).

## Alternatives

**An object-valued `display`.** For example
`[style.reasoning.display.ephemeral]` with `timer` and `rows`.
Rejected: it needs custom deserialization and key-value assignment for an enum
that is a string in every other case, and it fixes the window's content to
`full`.
`truncate` and `summary` could not go into a window.

**A new display variant, `display = "window"`.** Rejected for the same reason:
the content is fixed to `full`, and the variant duplicates the `timer` code path
and its arms in the separation and turn-view logic.

**Split `display` into a content mode and a placement.** This is the cleanest
model and removes the "no effect" combinations, but it migrates a public key for
no capability `window` does not already provide.
Worth revisiting if a third placement appears.

**Line assembly and draw-time wrapping in the printer.** A `LineSink::write`
with an open partial line per source, and a wrap mode in the worker.
Rejected for this feature: the formatter already assembles and wraps reasoning
rows before they reach the printer.
Both would help child stderr (long log lines, `\r` progress output) and can be
proposed on those merits.

## Non-Goals

- **Keeping reasoning after it ends.** A `keep = N` setting that prints the last
  N window rows persistently when the window closes is a natural follow-up.
  It needs the renderer to hold its own copy of the rows, since the region's
  buffer is private, and it turns windowed reasoning back into persistent
  output, which reopens the separation and tool-call shading decisions in
  [Rendering path](#rendering-path).
- **Windowing message text or tool output.** Only reasoning is routed to the
  window.
- **Changing the `display` modes.** Their behavior without a window is
  unchanged.
- **Implementing `summary`.** It is still unimplemented; once it lands, its
  output goes into the window like any other content.
- **Interacting with the window.** No scrolling, pausing, or expanding.
  The status region has no input handling ([RFD 091]).

## Risks and Open Questions

- **Default size.** `rows = true` sizes the window at a tenth of the terminal
  height, chosen in [RFD 091] for tool stderr: five rows on a 50-row terminal.
  That may be too little prose to follow.
  If so, the fix is a different divisor for this client, which needs a new
  `OutputLines` variant in `jp_printer`.
- **Flicker.** Each completed row repaints the whole window.
  Paragraph prose completes a row once per `wrap_width` columns (80 by default);
  code blocks and lists can complete rows faster.
  A burst of pushes raises one refresh per worker wakeup (`LineSink::push`).
  Whether that repaint rate flickers depends on the terminal and the token rate.
  Checked on a real terminal in phase 3 (Ghostty, with Anthropic's reasoning
  stream), the window does not flicker.
- **The window after an interrupt.** The flush that runs before the interrupt
  menu opens releases the window, so reasoning after *continue* starts in an
  empty window and the rows the user was reading when they pressed Ctrl-C are
  gone.
  Checked on a real terminal in phase 3, this does not read as lost context, so
  the window is not kept across the menu.
  Keeping it would mean leaving it claimed through that flush and through the
  reset that follows *continue*, the way `reset_preserving_region` already keeps
  the content region open across a stream retry.
  *Reply* starts a fresh window either way: it begins a new assistant section
  after a user message, and a reasoning region never survives a user message
  ([RFD 095]).

## Implementation Plan

1. **Rename `StderrRows` to `WindowRows`.** No behavior or config change.
   Mergeable on its own.

2. **Config.** Add `ReasoningWindowConfig` under `style.reasoning.window`,
   default off, with user-facing doc comments, and point the `display` doc
   comment at it.
   No renderer effect yet.
   Mergeable on its own.

3. **Renderer.** `Printer::window_rows`, the emission seam, the per-block
   placement, the window's lifetime and release, the region background,
   chrome-width wrapping, `timer` bookkeeping parity, and the live-flow gate.
   Tests in `render/chat_tests.rs` against an interactive memory printer cover:
   no reasoning bytes on stdout, rows pushed in order, the window surviving
   consecutive reasoning chunks, release at the tool-call boundary and at
   message start, drained reasoning not reaching stdout at either boundary, no
   shading on the following tool call, the background restored after an inline
   code span, replay ignoring the window, and inline output when the region is
   inert or would have no window rows.
   A screen-level test renders through `jp_pty`'s terminal model (the harness
   `jp_printer`'s region tests use) and asserts that the window is on screen
   while reasoning streams and gone once the message starts.
   Finished with a manual check on a real terminal for flicker, the default
   size, a resize mid-reasoning, and the window after an interrupt; the last one
   decides whether *continue* keeps the region.
   Depends on 2.

## References

- [RFD 091]: the status region, its rolling window, and its rendering
  conditions.
- [RFD 095]: reasoning-region shading across tool calls, which windowed
  reasoning must not trigger.
- [RFD 104]: per-channel terminal capability, which would change when the window
  renders under a redirected stdout.
- `crates/jp_cli/src/cmd/query/mcp_startup.rs`: the existing client that feeds a
  window from a stream of lines.
- `crates/jp_cli/src/render/chat.rs`: `render_reasoning`, the `Timer` display
  path, and `handle_paragraph_chunk`.

[RFD 091]: 091-printer-owned-status-region.md
[RFD 095]: 095-reasoning-region-shading-across-tool-calls.md
[RFD 104]: 104-terminal-capability-and-promptability-signals.md
