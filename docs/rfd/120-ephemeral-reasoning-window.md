# RFD 120: Ephemeral Reasoning Window

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-07
- **Extends**: [RFD 091]
- **Summary**: Streams rendered reasoning into a bounded status-region window
  that is erased once the assistant stops reasoning.

## Summary

This RFD adds `style.reasoning.window`, which streams rendered reasoning into a
fixed-height window at the bottom of the terminal while the assistant reasons
and erases the window when reasoning ends.
The window is built on the printer-owned status region ([RFD 091]) and composes
with the existing `style.reasoning.display` modes.

## Motivation

`style.reasoning.display` forces a choice between a noisy transcript and no
visibility into the assistant's reasoning:

- **Persistent** (`full`, `truncate`): prints reasoning into the transcript.
  A long reasoning block pushes the question, the tool calls, and the start of
  the answer off screen, leaving hundreds of lines in scrollback that are read
  once while they stream and never again.
- **Placeholder** (`static`, `progress`, `timer`, `hidden`): shows that the
  assistant is reasoning, but not what it is reasoning about.
  The user cannot tell whether the assistant understood the question or
  interrupt early when it heads in the wrong direction.

Neither mode lets the user watch reasoning while it happens and have it
disappear afterward.
JP already provides this behavior for child process output: while MCP servers
start (`style.mcp_startup.stderr_rows`) and while tools run
(`style.tool_call.stderr_rows`), the last few lines of the child's stderr scroll
through a bounded window above a status row and are erased before the next
persistent write.
Reasoning is the only stream of transient text without a rolling window.

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

The window renders rows the same way `display = "full"` does, with markdown
styling, wrapping, code highlighting, and `style.reasoning.background` shading.
As new rows complete, the oldest scroll out of the window.

When persistent output is about to render (message text, a tool call, the end of
the turn, or a stream retry), the window and its status row are erased, leaving
no reasoning on screen.
Resizing the terminal while the window is drawn is the only exception (see
[Drawbacks](#drawbacks)): narrowing a terminal that reflows its contents can
leave fragments of the window on screen, and shrinking a terminal below the
window's height pushes its top rows into scrollback, where the status region
cannot erase them.
Like any interactive prompt, the interrupt menu erases the window when it opens.
When reasoning resumes after the user selects *continue* or after a tool call
finishes, it opens a fresh, empty window.

The window governs live display only.
Reasoning is still recorded in the conversation stream, and `jp conversation
print --style full` replays it in full.

### Configuration

The `style.reasoning.window` section adds two keys:

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

`rows` accepts the same values as the existing `stderr_rows` keys and shares
their configuration type.

`window` controls where rendered reasoning content is placed, while `display`
controls what content is rendered:

| `display`                     | `window.rows = false` | `window.rows` set, window renders                |
| ----------------------------- | --------------------- | ------------------------------------------------ |
| `full`                        | inline, persistent    | all reasoning, in the window                     |
| `truncate` (`<number>`)       | first N chars inline  | first N chars in the window, then the status row |
| `summary`                     | summary inline        | summary in the window                            |
| `hidden`                      | nothing               | nothing; the window has no effect                |
| `static`, `progress`, `timer` | placeholder           | placeholder; the window has no effect            |

`display` conflates two concerns: how much reasoning text to render (`full`,
`truncate`, `summary`) and what to show instead of reasoning text (`hidden`,
`static`, `progress`, `timer`).
Because the placeholder and `hidden` modes produce no reasoning text, `window`
has no effect when they are active.

Combinations where `window` has no effect are accepted silently rather than
rejected as configuration errors.
Because configuration is layered, a user can set `window.rows = 8` globally and
pass `--cfg style.reasoning.display=hidden` for a single conversation; rejecting
that combination would prevent the override.
The doc comments on `display` and `window.rows` document how the two keys
interact.

### When the window cannot render

A status region renders only on an interactive stderr, with a pretty output
format, and with no tracing layer writing to stderr ([RFD 091]).
Outside those conditions, claiming a status region returns an inert handle.
Even when a status region is active, it can have zero window rows: when the
terminal height is unknown, or when `rows = true` resolves to zero on a terminal
shorter than ten rows, such as a small split pane ([RFD 091]).

In both cases, the renderer falls back to inline rendering: reasoning renders
according to `display`, using the same spacing, background shading, and output
channel as when `window.rows = false`.
Because `window` is a placement preference rather than a visibility setting, an
environment that cannot display the window still displays the reasoning.

This fallback covers stdout redirection: `jp query > answer.md` resolves
`--format auto` to `text`, which disables status regions ([RFD 091]) and writes
reasoning to `answer.md` just as it does without a window.
Under the current shared `--format` resolution, stdout redirection disables
status regions on stderr as well; if [RFD 104] lands, per-channel capability
detection will instead determine window rendering and row styling from stderr's
capabilities.

Because windowed and inline reasoning format rows differently (differing in wrap
width and background fill), the renderer chooses a placement once per reasoning
block before formatting its first row.
It queries `Printer::window_rows`, a read-only accessor that resolves `rows`
against the current terminal height and returns zero when status regions are
disabled or no window rows fit.
A non-zero count places the block in the window; zero places it inline.
That placement holds for the duration of the block: if a terminal resize reduces
the available window rows to zero mid-block, the status row remains on screen
and the rest of the block is not redirected inline.

### Rendering path

Apart from `Printer::window_rows`, no printer changes are needed.
The chat renderer already produces the line-oriented output that `LineSink`
accepts.

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
completed visual lines, holding back the in-progress line until it wraps or the
paragraph ends.
`print_block` and `print_code` emit complete rendered blocks.
Output reaching the printer therefore consists of styled, pre-wrapped lines
ready for `LineSink::push`.
Code blocks are the only exception: `Formatter::render_code_line` highlights
each line without wrapping it, so a code line wider than the terminal is
truncated at the window's right edge.

The renderer requires seven changes:

1. **Single emission seam.** Reasoning reaches the printer from `print_block`,
   `handle_paragraph_chunk`, `print_code`, and `emit_pending_separator`.
   All four call a single emission method that either writes to the printer or
   splits the text on `\n` and pushes each complete row to the window.
   Any trailing fragment without a terminating `\n` is buffered and prepended to
   the next emission so partial rows never enter the window; when the window is
   released, any buffered fragment is discarded.
   Message content is never routed to the window.

2. **Claim, lifetime, and release.** The window is a `StatusRegion` claimed on
   the first reasoning chunk of a block and stored in the same field as the
   timer region (`reasoning_timer`, renamed to cover both modes).
   A separate field records whether the block's placement is windowed or inline.
   Emission routing reads this placement field rather than `is_active()`, so
   releasing the window does not cause remaining chunks of the block to print
   inline.

   The window remains claimed across consecutive reasoning chunks:
   `flush_on_transition` releases the window only when transitioning to
   non-reasoning content.
   In `display = "timer"` mode, `flush_on_transition` still releases the region
   unconditionally, because the timer path never records a content kind.

   All other sites that release the timer also release the window: `flush`,
   `flush_for_continuation`, the tool-call boundary, `skip_tool_call`, and
   `reset`.
   Each site releases the region before draining the markdown buffer.
   Any windowed reasoning drained after the release is discarded rather than
   printed: it belongs to the erased window, and the event builder has already
   recorded it in the conversation stream.

   Claiming the window uses the same 300ms reveal delay as the timer: reasoning
   shorter than 300ms never flashes a window on screen, and longer reasoning
   appears with its initial rows already buffered.

3. **Live flow only.** The renderer claims a window only during
   `RenderFlow::Live`.
   Replay ignores `window` and renders reasoning inline according to `display`.

4. **Wrap width.** Because windowed rows render on stderr, prose wraps at
   `style.markdown.wrap_width` capped by the chrome channel's width
   (`printer.chrome_columns()`) rather than stdout's width
   (`printer.output_width()`).
   When stdout is redirected, `printer.output_width()` returns no width and the
   cap would otherwise not apply.
   Inline reasoning continues to wrap against stdout's width.

5. **Background.** On a terminal, `style.reasoning.background` is normally drawn
   via a `\x1b[K` line fill emitted by the formatter (`line_fill` in
   `jp_term::background`), but the status region's line filter strips all
   non-SGR escapes, including `\x1b[K`.
   Windowed reasoning is instead formatted with the reasoning background color
   and `BackgroundFill::Content`, which omits the `\x1b[K` fill.
   The formatter applies the background color behind text and restores it after
   inline elements that set their own background, such as inline code spans
   closing with `\x1b[49m`.
   The status region fills the rest of each row via
   `StatusRegion::background()`, which instructs the printer worker to shade
   every row it draws, including the status row.

6. **Bookkeeping matches `timer`.** Windowed reasoning is ephemeral chrome that
   leaves nothing in the transcript, so transcript bookkeeping ignores it:

   - `reasoning_supplies_separation` returns `false`.
   - `set_response_kind(Reasoning)` is not called, so `extend_across_tool_calls`
     ([RFD 095]) does not shade the next tool call; there is no persistent
     reasoning block around the tool call to extend.
   - No separator is deferred past the window.
     Blank lines between reasoning blocks become empty rows inside the window,
     and the trailing separator at the end of a reasoning block is dropped.

7. **No typewriter delay.** `LineSink::push` bypasses the typewriter effect,
   which paces persistent transcript writes rather than status previews.

### Configuration types

`StderrRows` (`jp_config::style::stderr_rows`) is renamed to `WindowRows` in a
preparatory change so its name and doc comments are not specific to child
processes.
The existing `stderr_rows` keys and their serialized values do not change.

`ReasoningWindowConfig` defines `rows: WindowRows` (default `Off`) and `timer:
bool` (default `true`), nested under `ReasoningConfig` as `window`, with the
standard `AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and `ToPartial`
implementations.
The renderer maps `rows` onto the printer's `OutputLines` using the existing
`output_lines` helper in `render/tool.rs`.

## Drawbacks

- **Two keys interact.** Some `display` × `window` combinations have no effect.
  Users scanning the `display` values will not discover `window` unless they
  read the doc comments linking the two settings.
- **Reasoning cannot be scrolled back to during the session.** Once the window
  is erased, the only way to read the reasoning is `jp conversation print
  --style full`.
  A future `keep` setting (see [Non-Goals](#non-goals)) would mitigate this.
- **The same config renders differently by environment.** With a window
  configured, reasoning is ephemeral in an interactive terminal and persistent
  under `-v`, `--log`, a non-terminal stderr, or a non-pretty output format,
  because each of those disables status regions.
  A user who configures a window to keep scrollback clean still gets persistent
  reasoning in scrollback when debugging with `-v`.
- **Resizing the terminal can leave parts of the window behind.** The formatter
  wraps rows once when rendering them, and a redraw after the terminal narrows
  truncates each row to the new width to preserve [RFD 091]'s every-row-fits
  invariant.
  However, a terminal that reflows its contents on resize (such as Ghostty) also
  re-wraps the rows already drawn on screen, background shading included.
  The on-screen window then spans more physical rows than the status region
  moves up to erase, so each redraw while the terminal narrows leaves fragments
  of the previous frame on screen.
  Manual testing in phase 3 reproduced this artifact, which the tool and MCP
  startup windows also share.
  Inline reasoning is hard-wrapped the same way, but because nothing erases it,
  a reflow only re-wraps it.
  Shrinking a terminal below the window's height can also push the window's top
  rows into scrollback, where the status region cannot erase them ([RFD 091]).
- **Long code lines are truncated.** When rendered inline, a code line wider
  than the terminal soft-wraps; in the window, it is truncated at the terminal
  edge because soft-wrapping would invalidate the status region's row count
  ([RFD 091]).

## Alternatives

**An object-valued `display` setting** (for example,
`[style.reasoning.display.ephemeral]` with `timer` and `rows`).
This approach requires custom deserialization and key-value assignment for an
enum whose other variants are plain strings, and it fixes the window's content
to `full` so `truncate` and `summary` cannot use the window.

**A new display variant, `display = "window"`.** Like an object-valued
`display`, a dedicated variant fixes the window's content to `full`.
It also duplicates the `timer` code path and its match arms in the separation
and turn-view logic.

**Splitting `display` into a content mode and a placement.** Separating content
selection from placement removes the no-op config combinations, but it migrates
a public config key without adding any capability beyond `window`.
If a third placement mode is introduced later, this split is worth revisiting.

**Line assembly and draw-time wrapping in the printer** (`LineSink::write` with
an open partial line per source, and a wrap mode in the printer worker).
The markdown formatter already assembles and wraps reasoning rows before they
reach the printer, so printer-level line assembly is unnecessary here.
Both features would help child stderr (wrapping long log lines and handling `\r`
progress output) and can be proposed separately for that use case.

## Non-Goals

- **Keeping reasoning after it ends.** A `keep = N` setting that prints the last
  N window rows persistently when the window closes is a natural follow-up.
  Supporting it requires the renderer to buffer its own copy of the rows
  (because the status region's buffer is private) and turns windowed reasoning
  back into persistent output, reopening the separation and tool-call shading
  decisions in [Rendering path](#rendering-path).
- **Windowing message text or tool output.** Only reasoning is routed to the
  window.
- **Changing the `display` modes.** Their behavior without a window is
  unchanged.
- **Implementing `summary`.** `display = "summary"` remains unimplemented; once
  implemented, its output will route to the window like any other content.
- **Interacting with the window.** The window does not support scrolling,
  pausing, or expanding, because the status region has no input handling ([RFD
  091]).

## Risks and Open Questions

- **Default size.** `rows = true` sizes the window at one tenth of the terminal
  height (five rows on a 50-row terminal), matching the default chosen in [RFD
  091] for tool stderr.
  If five rows is too small for following streaming prose, `jp_printer` can add
  a dedicated `OutputLines` variant with a different divisor for reasoning.
- **Flicker.** Each completed row repaints the entire window.
  Paragraph prose completes a row once per `wrap_width` columns (80 by default),
  while code blocks and lists can complete rows faster.
  A burst of pushes triggers one refresh per worker wakeup (`LineSink::push`),
  and whether that repaint rate flickers depends on the terminal and token rate.
  In manual testing during phase 3 on Ghostty with Anthropic's reasoning stream,
  the window did not flicker.
- **The window after an interrupt.** The flush that runs before the interrupt
  menu opens releases the window, so reasoning after *continue* starts in an
  empty window and the rows the user was reading when they pressed Ctrl-C are
  cleared.
  In manual testing during phase 3, clearing the window on interrupt did not
  read as lost context, so the window is not preserved across the menu.
  Preserving it would require keeping the status region claimed across both the
  pre-menu flush and the post-*continue* reset, similar to how
  `reset_preserving_region` keeps the content region open across a stream retry.
  Selecting *reply* always starts a fresh window: it begins a new assistant
  section after a user message, and a reasoning region never survives a user
  message ([RFD 095]).

## Implementation Plan

1. **Rename `StderrRows` to `WindowRows`.** Pure refactoring with no behavior or
   config change, mergeable on its own.

2. **Config.** Add `ReasoningWindowConfig` under `style.reasoning.window`
   (defaulting to off) with user-facing doc comments, and point the `display`
   doc comment at it.
   This phase has no renderer effect yet and is mergeable on its own.

3. **Renderer (depends on phase 2).** Implement `Printer::window_rows`, the
   emission seam, per-block placement, the window's claim and release lifecycle,
   the region background, chrome-width wrapping, `timer` bookkeeping parity, and
   the live-flow gate.
   Unit tests in `render/chat_tests.rs` against an interactive memory printer
   verify that: no reasoning bytes are written to stdout; rows are pushed in
   order; the window survives consecutive reasoning chunks; the window is
   released at the tool-call boundary and at message start without leaking
   drained reasoning to stdout; the following tool call is not shaded; the
   background is restored after an inline code span; replay ignores the window;
   and reasoning falls back to inline output when the region is inert or has no
   window rows.
   A screen-level test renders through `jp_pty`'s terminal model (the same
   harness used by `jp_printer`'s region tests) and asserts that the window is
   on screen while reasoning streams and erased once the message starts.
   Complete the phase with a manual check on a real terminal for flicker, the
   default size, a resize mid-reasoning, and the window behavior after an
   interrupt (which determines whether *continue* preserves the region).

## References

- [RFD 091]: the status region, its rolling window, and its rendering
  conditions.
- [RFD 095]: reasoning-region shading across tool calls, which windowed
  reasoning must not trigger.
- [RFD 104]: per-channel terminal capability detection, which would determine
  whether the window renders when stdout is redirected.
- `crates/jp_cli/src/cmd/query/mcp_startup.rs`: the existing status-region
  client that streams lines into a rolling window.
- `crates/jp_cli/src/render/chat.rs`: `render_reasoning`, the `Timer` display
  path, and `handle_paragraph_chunk`.

[RFD 091]: 091-printer-owned-status-region.md
[RFD 095]: 095-reasoning-region-shading-across-tool-calls.md
[RFD 104]: 104-terminal-capability-and-promptability-signals.md
