# RFD 095: Reasoning-Region Shading Across Tool Calls

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-25
- **Extends**: [RFD 048]
- **Summary**: Extends reasoning background shading across interleaved tool call
  headers, arguments, and results during a reasoning span.

## Summary

When the assistant interleaves reasoning with tool calls, JP treats those tool
calls as part of the surrounding reasoning region.
It extends the reasoning background across the tool chrome (header, arguments,
progress, and results) instead of dropping the shading for each tool call and
restoring it on the next reasoning block.
Shaded chrome preserves any background the tool's own output sets.
The behavior is configurable and enabled by default.

## Motivation

With `style.reasoning.background` set, reasoning content renders with a
full-width background fill that visually distinguishes it from the assistant's
answer.
Models with interleaved thinking (reasoning, then a tool call, then more
reasoning) break this visual continuity: each tool call drops the shading and
the next reasoning block picks it back up, so the background flips on and off
down the screen.

During interleaved thinking, the model calls tools as part of its reasoning
rather than stopping to do unrelated work and starting over.
The terminal output should render one continuous reasoning region that contains
those tool calls, rather than leaving the shading fragmented across multi-step
reasoning turns.

## Design

### What the user sees

A reasoning region is a contiguous span of an assistant turn that begins at the
first reasoning content and persists across any interleaved tool calls.
It ends when the assistant emits ordinary message content (the answer) or when
the turn ends.
**A tool call belongs to the reasoning region when the immediately preceding
chat response was reasoning.**

Within a reasoning region, the configured reasoning background fills the tool
chrome: the `Calling tool …` header, the argument preview, the progress line,
and the inline result.
The fill acts as a default background.
Where the tool's own output sets a background (such as a syntax theme or a
sub-command that emits its own SGR sequences), that background is preserved and
the region fill is suppressed for those spans.

Region shading applies only when reasoning produces persistent shaded content,
namely the `full` and `<number>` (truncate) display modes.
`progress`, `static`, and `timer` write ephemeral chrome with no shaded region
to extend, so they are unaffected.
`summary` is unimplemented; if it later renders persistent reasoning content, it
should follow the same rule.

### Configuration

Add `extend_across_tool_calls` under `style.reasoning`, defaulting to `true`:

```toml
[style.reasoning]
background = 236
# Extend the reasoning background across tool calls made while reasoning.
extend_across_tool_calls = true
```

The flag gates only the extension across tool calls.
When `background` is unset, there is no fill to extend and the flag is a no-op.
Setting `background` and leaving `extend_across_tool_calls` at `true` produces
the continuous region; setting `extend_across_tool_calls` to `false` shades only
the reasoning blocks themselves.

### Cross-channel rendering

Reasoning and tool chrome are rendered by two different components writing to
two different channels (RFD 048):

- `ChatRenderer` writes reasoning and message content to **stdout** and owns the
  background fill via `jp_md`'s `DefaultBackground` and `TerminalOptions`.
- `ToolRenderer` writes the tool header, arguments, progress, and results to
  **stderr** with direct `write!` calls and previously had no background
  concept.

The two renderers are siblings: in the live path, `turn_loop` owns
`TurnCoordinator` (which owns `ChatRenderer` via `TurnView`) and `ToolRenderer`
separately; in replay, `TurnRenderer` owns both.
On an interleaved terminal (the common `jp q …` case), stdout and stderr
display together, so unshaded stderr chrome creates a visible gap in an
otherwise shaded region.

The channel model already handles stream redirection: non-pretty output strips
ANSI sequences on both stdout and stderr, and `--format auto` resolves to
non-pretty when stdout is not a terminal, so `jp q > out 2> err` never emits the
fill on either stream.

Extending the region across both renderers requires deciding when a tool call
belongs to a reasoning region and maintaining the background fill across
`ToolRenderer`'s byte stream.

### Region membership

Region membership depends on the kind of the last **chat response** (reasoning
vs. message), which a tool call must read without overwriting.
Because the tool-call transition updates `last_content_kind` (previously the
only record of the active content kind), `ChatRenderer` keeps a separate
`last_response_kind` field that preserves the last chat-response kind across
tool calls.

Membership is decided **per tool call at the request boundary and captured by
tool-call ID**, rather than held as a single renderer-wide flag.
A tool call's header renders during streaming, while its result renders later in
the execution phase, where several tool results are emitted in sequence after
the stream ends.
By then there is no single current region, so the request boundary records the
region background under the tool-call ID for result rendering to look up:

- Live: at `ToolCallPart::Start` (the `enter_tool_call` boundary), record the
  region background under the tool-call ID.
- Replay: `TurnRenderer` already keys `tool_names: HashMap<id, name>`; a
  parallel `HashMap<id, Option<DefaultBackground>>` captures the region when
  `ToolCallRequest` renders and is consumed when the matching `ToolCallResponse`
  renders.

`enter_tool_call` both flushes the pending separator and returns the region
background.
Reading the region state after the buffer flush would be too late, because the
flush drains the deferred reasoning separator.
`enter_tool_call` therefore decides whether the region is shaded *before*
draining, emits the separator accordingly (shaded when the region continues
across the tool call, unshaded when reasoning gave way to a message), and
returns the region background for the tool renderer.

**A tool call is transparent to the region when it shows no chrome.** Chrome can
be suppressed per tool (`style.hidden = true`) or globally
(`style.tool_call.show = false`, which hands `ToolRenderer` a sink at
`turn_loop.rs:178`).
In the live path, `render_approved_tool` and result rendering also return early
on `is_hidden` (`tool/coordinator.rs:658`, `:1079`).
Because none of these cases emit chrome, the boundary checks a single visibility
predicate rather than the per-tool flag alone:

```rust
let tool_chrome_visible =
    cfg.style.tool_call.show && !printer.format().is_json() && !tool_style.hidden;
```

When `tool_chrome_visible` is `false`, the boundary is transparent: it leaves
the pending reasoning separator undrained, preserves `last_response_kind`, and
records no region metadata.
The separator decision defers to the next *visible* content, so a hidden tool
between two reasoning blocks leaves the region intact.
(In JSON mode, the chat renderer is already a sink at `turn/coordinator.rs:184`,
so `!is_json` only governs the tool renderer.)

Live and replay use the same `show && !json && !hidden` predicate.
In replay, `TurnRenderer` previously built `ToolRenderer` with the real printer
unconditionally (`turn.rs:74-81`, `:193-199`) and gated only on per-tool
`hidden` (`turn.rs:126`, `:128`, `:153`), causing `jp conversation print` to
ignore `style.tool_call.show` and the JSON format.
The replay phase wires `tool_chrome_visible` into `TurnRenderer`, fixing that
bug and bringing replay to parity with the live path.

**The live boundary runs once, owned by `turn_loop`.** Previously, the live path
entered tool-call mode in two places for the same `ToolCallPart::Start`: once in
`turn_loop` and again in `TurnCoordinator::handle_streaming_event`.
That duplication was harmless only while the transition was idempotent.
Once the boundary drains the separator and captures per-ID region state, it must
run exactly once.
`turn_loop` owns the transition because it alone holds the tool ID, tool name,
`cfg`, and `ToolRenderer` needed to compute `tool_chrome_visible` and record
region metadata by ID; `TurnCoordinator` no longer fires the boundary on tool
starts.

### Enforcing the background invariant with `ShadedWriter`

Shading maintains a **terminal background invariant** across the byte stream:
while a region with background `B` is active, every visual line displays `B`
from column 0 to the right edge, except across spans where the content sets its
own background.
This invariant also covers lines updated by cursor-relative rewrites, such as
the `\r`-redrawn `Calling tool …` temp line and the `⏱ Running…` progress
line, which can stay on screen for tens of seconds.

`jp_md` already enforces this invariant for markdown lines: `AnsiState`
(`ansi.rs`) tracks the content's active background (`48;…`) and
`TerminalWriter` applies a `DefaultBackground`, restoring it around spans that
set their own.
That logic is `pub(crate)` and coupled to the line-wrapping pipeline.
This RFD extracts it into a reusable, stateful **`ShadedWriter`** decorator:

```rust
/// Wraps a writer and maintains a default-background invariant across the byte
/// stream, preserving any background the content sets itself.
/// The tracked `AnsiState` persists across writes.
pub struct ShadedWriter<W: Write> { /* … */ }

/// Convenience: run a `ShadedWriter` over a buffer.
/// Used by replay and tests; the live path uses the streaming decorator
/// directly.
pub fn shade(text: &str, background: &DefaultBackground) -> String;
```

`ShadedWriter` enforces the invariant as bytes flow through:

- **At each logical line start** (stream start, after `\n`, or after `\r`):
  assert `B` if no content background is active, so a following `\x1b[K` erases
  with `B` and following text sits on `B`.
- **Before forwarding a content `\x1b[K`**: assert `B` first **only when no
  content background is active**, so the erase fills with `B`; when the content
  has its own background, leave it active so the erase preserves the content's
  fill.
- **Before a `\n`**: emit `\x1b[K` to fill to the right edge with the current
  background (using the same `\x1b[{param}m\x1b[K\x1b[49m` sequence as
  `render_separator`).
- **After content emits `\x1b[0m` or `\x1b[49m`**: re-assert `B` so the region
  survives a content reset.
- **When content sets its own `48;…` background**: suppress `B` for that span
  and restore `B` when the content resets its background (tracked by
  `AnsiState`).
- **At region end**: emit `\x1b[49m` so `B` does not leak past the region.

`ShadedWriter` persists its state across writes for the same reason
`jp_printer`'s `AnsiStripper` persists its `vte::Parser`: an escape sequence or
line boundary can split across `write!` calls.
The pure `shade` function wraps `ShadedWriter` to shade an in-memory buffer
using the same state machine.

Because tool chrome is arbitrary text and ANSI sequences rather than markdown,
routing it through the markdown formatter would misparse lines such as `# foo`
as a heading and `* x` as a bullet.
Sharing `ShadedWriter` reuses the SGR-aware background tracking without invoking
the markdown parser.

`ToolRenderer` activates a `ShadedWriter` around the writes for a given tool
when that tool's captured region background is `Some`, and writes straight to
`err_writer()` otherwise.
Its existing `write!` and `writeln!` sites (header, argument preview,
`\r`-rewritten temp/progress line, `\r\x1b[K` clear on completion, and result)
flow through unchanged.
Tool-result code blocks already pass through the formatter's `render_code_line`,
so they take the background parameter directly.
Because the temp/progress line is a live aggregate of the currently pending
tools, it uses the active region; each tool's permanent header and result use
that tool's captured region.

### `ShadedWriter` contract

The writer acts on two escape families: SGR (`\x1b[…m`) and the CSI erase
`\x1b[K`.
Any other escape is forwarded verbatim without affecting the invariant, and a
malformed or split sequence is held by the persistent parser until it completes,
matching `AnsiStripper`.
Forwarding escapes verbatim requires recognizing each escape as a complete unit:
OSC string sequences (`\x1b]…` terminated by BEL or ST), particularly the OSC 8
hyperlinks emitted in tool results, must be tokenized end-to-end so they pass
through intact while their visible link text remains shaded.
The tokenizer cannot stop an OSC sequence at the first letter the way it does
for a CSI sequence.

`ShadedWriter` parses **compound** SGR parameters, not only standalone
`\x1b[48;…m`.
Real tool output combines attributes (such as `\x1b[1;48;5;236m`,
`\x1b[38;2;…;48;2;…m`, `\x1b[0;48;5;236m`, and `\x1b[39;49m`), and matching
only a leading `48;` would miss the content's background and overwrite it.
Previously, `AnsiState` matched only a leading `48;` or `38;`; phase 2 extends
it to scan each `;`-separated sub-parameter for `48`, `49`, `38`, and `39`.

When content emits `\x1b[K`, `ShadedWriter` fills with the **content's**
background when one is active, and with the region background otherwise.
A tool that sets its own background and clears to end of line keeps its own
fill, while the region background backs only spans that the content left at
default.
At the end of the region, the writer emits `\x1b[49m` so `B` never leaks past
it.

### Data flow

```text
reasoning chunk ─▶ ChatRenderer (stdout, shaded)
                     │ remembers: last chat response = Reasoning
                     ▼
tool A start    ─▶ enter_tool_call: shade pending separator, return Some(bg)
                     │ capture region[A] = Some(bg)
                     ▼
                  ToolRenderer: A's writes go through ShadedWriter(bg)
                     │ header / args / temp+progress line / result
                     ▼ (stderr, shaded; content's own bg preserved)
reasoning chunk ─▶ ChatRenderer (stdout, shaded; region continues)
message chunk  ─▶ ChatRenderer (stdout, unshaded; region ends)
tool B start    ─▶ capture region[B] = None  (followed a message)
                     ▼ B's result later renders unshaded; A's stays shaded
```

## Drawbacks

- Shading stderr chrome with the reasoning background couples the chrome channel
  to an assistant-output styling decision.
  In the split-redirect case (`jp q 2> err` with stdout on a TTY), the
  redirected stderr receives the background fill, though that stream already
  carries chrome ANSI sequences (such as yellow tool names), so the coupling is
  pre-existing.
- The region decision lives in `ChatRenderer` while the per-tool-call-ID capture
  and application live in `ToolRenderer`, and both must stay consistent across
  the live and replay paths.
- Exposing `ShadedWriter` widens `jp_md`'s public API (Hyrum's Law) for a
  capability that previously had a single internal caller; its background
  invariant becomes an observable contract.

## Alternatives

- **Shared shading context lifted to the coordinator.** Have `turn_loop` and
  `TurnRenderer` own a single `ShadingState` and thread it into both renderers.
  This provides a single source of truth, but the region decision still
  originates in `ChatRenderer`'s content-kind tracking, and a single downstream
  consumer does not justify the extra plumbing.
  Revisit if a third consumer of the region appears (such as structured output
  or inquiry chrome).
- **Route chrome through the markdown formatter.** Rejected: tool output and
  argument previews are not markdown; parsing them would mangle their content.
  Only the SGR-aware background primitive should be shared.
- **Shade permanent chrome only; leave the progress/temp line unshaded.**
  Rejected: a streaming tool call holds the `\r`-redrawn line on screen for tens
  of seconds, leaving a long-lived unshaded gap in the region and preserving the
  visual flicker this feature removes.
- **A pure line-oriented `shade_lines(&str)`.** Rejected: a line-oriented
  function cannot maintain the background across `\r` and `\x1b[K` cursor
  rewrites used by the progress and temp lines.
  Enforcing the invariant across the byte stream requires `ShadedWriter`, while
  the pure `shade` helper over a buffer remains available as a convenience built
  on the same core.

## Non-Goals

- Changing the reasoning display modes or the meaning of
  `style.reasoning.background`.
- Changing per-stream ANSI policy on redirect, which the four-channel model
  already handles.
- Shading non-reasoning regions, or shading tool calls that follow ordinary
  message content.

## Risks and Open Questions

- **Ordering.** The shaded stderr lines only abut the shaded stdout lines
  because `Printer` serializes both streams through one command channel.
  The implementation must not break that serialization.
- **Replay parity.** Both paths apply the same `tool_chrome_visible` predicate.
  Because replay previously ignored `show` and `json` (a bug), wiring the
  predicate into `TurnRenderer` is a user-visible fix in its own right:
  transcripts printed with `style.tool_call.show = false` stop rendering tool
  chrome.
  Call that out in the changelog when the replay phase lands.
- **Region end semantics.** The rule ends the region at the first message
  content.
  A model that emits message text *between* reasoning and a tool call ends the
  region early; this is the intended reading of "the previous chat response was
  reasoning," implemented as `last_response_kind` tracking in `ChatRenderer`.
- **Concurrent tools in one region.** The temp line aggregates all pending tools
  and uses the current region, while per-tool permanent output uses each tool's
  captured region.
  If two tools are registered under different regions (because a message arrived
  between them), the shared temp line reflects the current region, which matches
  its role as a live aggregate.
- **Shared `AnsiState`.** Extending `AnsiState` to parse compound SGR changes a
  type the markdown code path already uses for background restoration.
  Cover it with characterization tests on current markdown rendering *before*
  the change to avoid a silent regression.
- **Overlap with RFD 084.** [RFD 084] (Discussion) migrates
  `DefaultBackground::param` from a pre-built SGR string to a logical `Color`
  built inside `jp_md`.
  This is not a blocker: if RFD 084 lands first, `ShadedWriter` should take the
  post-084 type; if this RFD lands first, RFD 084 must include the new public
  surface in its migration.

## Implementation Plan

1. **Region memory and a single live boundary.** Track the last chat-response
   kind in `ChatRenderer` separately from `last_content_kind`.
   Make the tool-call boundary a single operation owned by `turn_loop`, driven
   by a `tool_chrome_visible` predicate (`show && !json && !hidden`): when
   visible, it decides the separator's shading before flushing and returns the
   region background; when not, it leaves the separator pending and captures
   nothing.
   Remove the duplicate `enter_tool_call` call from
   `TurnCoordinator::handle_streaming_event`.
   This phase is unit-testable and mergeable on its own, with no visible change
   until tool chrome consumes the returned region background.
2. **`ShadedWriter` in `jp_md`.** Lift the background-invariant logic out of
   `TerminalWriter` into a public stateful decorator plus a `shade` convenience
   function, and extend `AnsiState` to parse compound SGR parameters.
   Add characterization coverage on the existing markdown code path first, then
   test `\r` and `\x1b[K` rewrites, compound-SGR content backgrounds,
   erase-under-content-background, and mid-stream resets.
   This phase is mergeable independently.
3. **OSC-aware tokenization for shaded hyperlinks.** Teach the shared `segments`
   tokenizer (and `ShadedWriter`'s split-escape detection) to recognize OSC
   string sequences (`\x1b]…` terminated by `\x07` or `\x1b\\`) as complete
   escapes, so a tool result's OSC 8 hyperlinks pass through `ShadedWriter`
   verbatim while their visible link text is still shaded.
   Without this, the line-oriented tokenizer stops an OSC sequence at its first
   letter and the writer injects the region background into the middle of the
   URL, corrupting the link.
   Tests cover an OSC 8 hyperlink under shading, an OSC sequence split across
   writes, and a `segments`/`visual_width` round-trip on OSC sequences.
   This phase is mergeable independently and is a prerequisite for routing
   results through `ShadedWriter` in phase 4.
4. **Per-ID chrome shading in `ToolRenderer`.** Capture the region background by
   tool-call ID at the request boundary, and route all writes for that tool
   (including the full result) through a `ShadedWriter` when set.
   `render_code_line` continues to pass `None` so `ShadedWriter` remains the
   sole owner of the result's background.
   Depends on phases 1, 2, and 3.
5. **Config field.** Add `style.reasoning.extend_across_tool_calls` (`bool`,
   default `true`) alongside `style.reasoning.background`.
   The setting is inert when `style.reasoning.background` is unset and gates
   phase 4.
6. **Replay path.** Capture the per-ID region in `TurnRenderer` and apply the
   same `tool_chrome_visible` predicate as the live path, fixing the
   pre-existing bug where replay ignores `style.tool_call.show` and JSON format.
   Verify the shaded replay output with snapshot tests.

## References

- [RFD 048]: Four-Channel Output Model (stdout/stderr/tty/log split).
- `crates/jp_cli/src/render/chat.rs`: reasoning background, content-kind
  tracking, and deferred separator.
- `crates/jp_cli/src/render/tool.rs`: tool chrome rendering.
- `crates/jp_md/src/shade.rs`: the `ShadedWriter` decorator and `shade`.
- `crates/jp_md/src/writer.rs`, `crates/jp_md/src/ansi.rs`: `DefaultBackground`
  application and `AnsiState` background tracking.
- `crates/jp_config/src/style/reasoning.rs`: reasoning style config.

[RFD 048]: 048-four-channel-output-model.md
[RFD 084]: 084-configurable-markdown-element-coloring.md
