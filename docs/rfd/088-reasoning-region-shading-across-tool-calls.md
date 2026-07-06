# RFD 088: Reasoning-region shading across tool calls

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-25
- **Extends**: [RFD 048]

## Summary

When the assistant interleaves reasoning with tool calls, treat the tool calls
as part of the surrounding reasoning region: extend the reasoning background
across the tool chrome (header, arguments, progress, results) instead of letting
the shading flip off for each tool call and back on for the next reasoning
block.
Shaded chrome preserves any background the tool's own output already carries.
The behavior is configurable and defaults to on.

## Motivation

With `style.reasoning.background` set, reasoning content renders with a
full-width background fill that visually distinguishes it from the assistant's
answer.
Models with interleaved thinking (reasoning, then a tool call, then more
reasoning, repeated) break this: each tool call drops the shading and the next
reasoning block picks it back up, so the background flips on and off down the
screen.

That flip portrays the wrong mental model.
The model is *reasoning, and calling tools while it reasons* — not stopping its
reasoning, doing unrelated work, and starting over.
The visuals should portray one continuous reasoning region that happens to
contain tool calls.

Doing nothing leaves the shading fragmented for exactly the models whose
reasoning is most worth following.

## Design

### What the user sees

A reasoning region is a contiguous span of an assistant turn that begins at the
first reasoning content and persists across any interleaved tool calls.
It ends when the assistant emits ordinary message content (the answer) or the
turn ends.
The rule that decides membership is simple: **a tool call belongs to the
reasoning region when the immediately preceding chat response was reasoning.**

Within a reasoning region, the configured reasoning background now also fills
the tool chrome — the `Calling tool …` header, the argument preview, the
progress line, and the inline result.
The fill is applied as a *default* background: where the tool's own output
already sets a background (a syntax theme, a sub-command that emits its own
SGR), that background is preserved and the region fill is suppressed for those
spans.

This applies only when reasoning produces persistent shaded content — the
`full` and `<number>` (truncate) display modes.
`progress`, `static`, and `timer` write ephemeral chrome with no shaded region
to extend, so they are unaffected.
`summary` is currently unimplemented; if it later renders persistent reasoning
content, it should opt into the same rule in that follow-up.

### Configuration

A new boolean under `style.reasoning`, defaulting to `true`:

```toml
[style.reasoning]
background = 236
# Extend the reasoning background across tool calls made while reasoning.
extend_across_tool_calls = true
```

The flag gates only the *extension*.
With `background` unset there is no fill to extend, so the flag is a no-op;
setting `background` and leaving the flag at its default produces the continuous
region.
Setting the flag to `false` restores the current per-block behavior.

### Why this is non-trivial

Reasoning and tool chrome are rendered by two different components writing to
two different channels (RFD 048):

- `ChatRenderer` writes reasoning and message content to **stdout** and owns the
  background fill, via `jp_md`'s `DefaultBackground` / `TerminalOptions`.
- `ToolRenderer` writes the tool header, arguments, progress, and results to
  **stderr** with ad-hoc `write!`s and, before this RFD, had no background
  concept.

The two are siblings: in the live path `turn_loop` owns the `TurnCoordinator`
(which owns `ChatRenderer` via `TurnView`) and the `ToolRenderer` separately; in
replay `TurnRenderer` owns both.
On an interleaved terminal — the common `jp q …` case — stdout and stderr
display together, so the unshaded stderr chrome is the visible gap in an
otherwise shaded region.

The redirect cases are already handled by the channel model: non-pretty output
strips ANSI on both stdout and stderr, and `--format auto` resolves to
non-pretty when stdout is not a terminal, so `jp q > out 2> err` never sees the
fill on either stream.
No new behavior is needed there.

The problem splits into two independent concerns.

### The "when": region membership

The determining state is the kind of the last **chat response** (reasoning vs.
message), which a tool call must read but must not overwrite.
Before this RFD, the tool-call transition overwrote the only record that the
region was reasoning.
`ChatRenderer` now keeps a separate `last_response_kind` field that remembers
the last chat-response kind across tool-call interludes.

Membership is decided **per tool call, at the request boundary, and captured by
tool-call ID** — not held as a single renderer-wide flag.
A tool call's header is rendered while streaming, but its result is rendered
later, in the execution phase, where several tools' results are emitted in
sequence after the stream has ended.
By then the "current" region is meaningless, so the boundary captures the region
background for that tool ID and the result rendering looks it up:

- Live: at `ToolCallPart::Start` (the `enter_tool_call` boundary), record the
  region background under the tool-call ID.
- Replay: `TurnRenderer` already keys `tool_names: HashMap<id, name>`; a
  parallel `HashMap<id, Option<DefaultBackground>>` captures the region when the
  `ToolCallRequest` is rendered and is consumed when the matching
  `ToolCallResponse` renders.

The boundary itself is an operation, not a passive query.
A value read after the buffer flush would be too late, because the flush drains
the deferred reasoning separator.
So `enter_tool_call` decides shaded-vs-unshaded *before* draining, emits the
separator accordingly (shaded when the region continues across the tool call,
unshaded when reasoning gave way to a message), and returns the region
background to the caller for the tool renderer.

**A tool call is transparent to the region when it shows no chrome.** Chrome is
suppressed per-tool (`style.hidden = true`) and globally (`style.tool_call.show
= false`, which hands the `ToolRenderer` a sink at `turn_loop.rs:178`); in the
live path `render_approved_tool` and the result rendering also early-return on
`is_hidden` (`tool/coordinator.rs:658`, `:1079`).
All of these produce no chrome, so the boundary keys off a single predicate
rather than the per-tool flag alone:

```rust
let tool_chrome_visible =
    cfg.style.tool_call.show && !printer.format().is_json() && !tool_style.hidden;
```

When `tool_chrome_visible` is false the boundary is transparent: it does not
drain the pending reasoning separator, does not overwrite the last chat-response
kind, and records no region metadata to shade.
The separator decision defers to the next *visible* content, so an invisible
tool between two reasoning blocks leaves the region intact.
(In JSON mode the chat renderer is already a sink at `turn/coordinator.rs:184`,
so the `!is_json` term only governs the tool renderer; chat spacing is moot
there.)

The predicate is uniform across render paths: live and replay both answer *is
this tool's chrome visible?* with `show && !json && !hidden`.
Replay currently diverges: `TurnRenderer` builds the `ToolRenderer` with the
real printer unconditionally (`turn.rs:74-81`, `:193-199`) and gates only on
per-tool `hidden` (`turn.rs:126`, `:128`, `:153`), so `jp conversation print`
ignores `style.tool_call.show` and the JSON format.
That divergence is a bug, not a constraint to design around: the replay phase
wires the same predicate into `TurnRenderer`, fixing it as part of delivering
replay parity.

**The live boundary runs once, owned by `turn_loop`.** Before this RFD, the live
path entered tool-call mode in two places — a redundant call from `turn_loop`
and another from `TurnCoordinator::handle_streaming_event` for the same
`ToolCallPart::Start` — harmless only because the operation was idempotent.
Once the boundary drains the separator and captures per-ID region state it must
run exactly once, so `turn_loop` owns it: it alone holds the tool id, name,
`cfg`, and `ToolRenderer` needed to compute `tool_chrome_visible` and record
region metadata by id.
`TurnCoordinator` no longer fires the boundary on tool starts.

### The "how": a shading invariant, enforced by a writer

Shading is not a line transform.
The right model is a **terminal background invariant**: while a region with
background `B` is active, every visual line shows `B` from column 0 to the right
edge — including lines produced by cursor-relative rewrites (the `\r`-redrawn
`Calling tool …` temp line and the `⏱ Running…` progress line, which can stay
on screen for tens of seconds) — except spans where the content sets its own
background.

`jp_md` already enforces this invariant for markdown lines: `AnsiState`
(`ansi.rs`) tracks the content's active background (`48;…`) and
`TerminalWriter` applies a `DefaultBackground`, restoring it around spans that
set their own.
That logic is `pub(crate)` and welded to the wrap pipeline.
This RFD lifts it into a reusable, stateful **`ShadedWriter`** decorator:

```rust
/// Wraps a writer and maintains a default-background invariant across the
/// byte stream, preserving any background the content sets itself. The tracked
/// `AnsiState` persists across writes.
pub struct ShadedWriter<W: Write> { /* … */ }

/// Convenience: run a `ShadedWriter` over a buffer. Used by replay and tests;
/// the live path uses the streaming decorator directly.
pub fn shade(text: &str, background: &DefaultBackground) -> String;
```

`ShadedWriter` enforces, as bytes flow through:

- **At each logical line start** (stream start, after `\n`, after `\r`): assert
  `B` if no content background is active, so a following `\x1b[K` erases *with*
  `B` and following text sits on `B`.
- **Before forwarding a content `\x1b[K`**: assert `B` first **only when no
  content background is active**, so the erase fills with `B`; when the content
  has its own background, leave it so the erase preserves the content's fill.
- **Before a `\n`**: emit `\x1b[K` to fill to the right edge with the current
  background (the `\x1b[{param}m\x1b[K\x1b[49m` shape `render_separator` already
  uses).
- **After content emits `\x1b[0m` or `\x1b[49m`**: re-assert `B` so the region
  survives a content reset.
- **When content sets its own `48;…` background**: suppress `B` for that span,
  restore on the content's bg-off (tracked by `AnsiState`).
- **At region end**: emit `\x1b[49m` so `B` does not leak past the region.

The state persists across writes for the same reason `jp_printer`'s
`AnsiStripper` persists its `vte::Parser`: a sequence or line boundary can split
across `write!` calls.
The pure `shade` function is a thin wrapper that runs the decorator over an
owned buffer — one core, two surfaces.

The tool chrome is arbitrary text plus ANSI, **not** markdown — it must not be
routed through the markdown formatter, which would parse `# foo` as a heading
and `* x` as a bullet.
The SGR-aware writer is the correct shared mechanism; the markdown parser is
not.

`ToolRenderer` activates a `ShadedWriter` around the writes for a given tool
when that tool's captured region background is `Some`, and writes straight to
`err_writer()` otherwise.
Its existing `write!`/`writeln!` sites — header, argument preview, the
`\r`-rewritten temp/progress line, the `\r\x1b[K` clear on completion, and the
result — flow through unchanged; the decorator does the work.
Tool-result code blocks already pass through the formatter's `render_code_line`,
so they take the background param directly.
The temp/progress line is a live aggregate of the tools pending *now*, so it
uses the currently-active region; each tool's permanent header and result use
that tool's captured region.

### `ShadedWriter` contract

The writer acts on two escape families — SGR (`\x1b[…m`) and the CSI erase
`\x1b[K`.
Any other escape is forwarded verbatim and does not affect the invariant; a
malformed or split sequence is held by the persistent parser until it completes,
as `AnsiStripper` already does.
"Forwarded verbatim" requires recognizing each escape as a whole unit: OSC
string sequences (`\x1b]…` terminated by BEL or ST) — most importantly the OSC
8 hyperlinks a tool result emits — must be tokenized end-to-end so they pass
through intact while their visible link text is still shaded.
The tokenizer cannot stop an OSC at the first letter the way it does for a CSI
sequence.

It parses **compound** SGR parameters, not only standalone `\x1b[48;…m`.
Real tool output combines attributes — `\x1b[1;48;5;236m`,
`\x1b[38;2;…;48;2;…m`, `\x1b[0;48;5;236m`, `\x1b[39;49m` — and a writer that
matched only a leading `48;` would miss the content's background and shade over
it.
Before this RFD, `AnsiState` matched a leading `48;`/`38;` only; phase 2 extends
it to scan each `;`-separated sub-parameter for `48`/`49`/`38`/`39`.

Erase policy: a content `\x1b[K` fills with the **content's** background when
one is active, and with the region background otherwise — so a tool that sets
its own background and clears to end-of-line keeps its own fill, and the region
background only backs spans the content left at default.
The writer emits `\x1b[49m` at region end so `B` never leaks past it.

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
  The split-redirect case (`jp q 2> err` with stdout a TTY) carries the fill
  onto the redirected stderr — but that stream already carries chrome ANSI
  (yellow tool names) today, so the coupling is pre-existing, not introduced
  here.
- The region decision lives in `ChatRenderer` while the per-tool-call-ID capture
  and application live in `ToolRenderer`, and both have to stay consistent
  across the live and replay paths.
- Exposing `ShadedWriter` widens `jp_md`'s public API (Hyrum's Law) for a
  capability that previously had a single internal caller; its background
  invariant becomes an observable contract.

## Alternatives

- **Shared shading context lifted to the coordinator.** A single `ShadingState`
  owned by `turn_loop` / `TurnRenderer`, threaded into both renderers.
  This is the cleaner single-source-of-truth shape, but the region decision
  still originates in `ChatRenderer`'s content-kind tracking, and the threading
  is more than one consumer justifies today.
  Revisit if a third consumer of the region appears (e.g. structured output or
  inquiry chrome).
- **Route chrome through the markdown formatter.** Rejected: tool output and
  argument previews are not markdown; parsing them would mangle their content.
  Only the SGR-aware background primitive is worth sharing.
- **Shade permanent chrome only; leave the progress/temp line unshaded.**
  Rejected: a streaming tool call holds the `\r`-redrawn line on screen for tens
  of seconds, so this leaves the *longest-lived* unshaded gap in the region —
  exactly the flicker the feature exists to remove.
- **A pure line-oriented `shade_lines(&str)`.** Rejected: it cannot maintain the
  background across `\r`/`\x1b[K` cursor rewrites, which is precisely the
  progress/temp-line case.
  The invariant has to be enforced across the byte stream, which is what
  `ShadedWriter` does; a pure `shade` over a buffer stays available as a
  convenience built on the same core.

## Non-Goals

- Changing the reasoning display modes or the meaning of
  `style.reasoning.background`.
- Per-stream ANSI policy on redirect — already handled by the four-channel
  model.
- Shading non-reasoning regions, or shading tool calls that follow ordinary
  message content.

## Risks and Open Questions

- **Ordering.** The shaded stderr lines only abut the shaded stdout lines
  because the `Printer` serializes both streams through one command channel.
  The implementation must not break that serialization.
- **Replay parity.** Both paths apply the same `tool_chrome_visible` predicate.
  Because replay today ignores `show`/`json` (a bug), wiring the predicate into
  `TurnRenderer` is a user-visible fix in its own right: transcripts printed
  with `style.tool_call.show = false` stop rendering tool chrome.
  Call that out in the change log when the replay phase lands.
- **Region end semantics.** The rule ends the region at the first message
  content.
  A model that emits message text *between* reasoning and a tool call ends the
  region early; this is the intended reading of "the previous chat response was
  reasoning," implemented as `last_response_kind` tracking in `ChatRenderer`.
- **Concurrent tools in one region.** The temp line aggregates all pending tools
  and uses the current region; per-tool permanent output uses each tool's
  captured region.
  If two tools were registered under different regions (a message arrived
  between them), the shared temp line reflects the current one — acceptable,
  since it is inherently a live aggregate.
- **Shared `AnsiState`.** Extending `AnsiState` to parse compound SGR changes a
  type the markdown code path already uses for background restoration.
  Cover it with characterization tests on current markdown rendering *before*
  the change, to avoid a silent regression there.
- **Overlap with RFD 084.** [RFD 084] (Discussion) migrates
  `DefaultBackground::param` from a pre-built SGR string to a logical `Color`
  built inside `jp_md`.
  Not a blocker, but if RFD 084 lands first, `ShadedWriter` should take the
  post-084 type; if this RFD lands first, RFD 084 must include the new public
  surface in its migration.

## Implementation Plan

1. **Region memory and a single live boundary.** Track the last chat-response
   kind in `ChatRenderer` separately from `last_content_kind`.
   Make the tool-call boundary one operation owned by `turn_loop`, driven by a
   `tool_chrome_visible` predicate (`show && !json && !hidden`): when visible it
   decides the separator's shading before flushing and returns the region
   background; when not, it leaves the separator pending and captures nothing.
   Remove the duplicate `enter_tool_call` call from
   `TurnCoordinator::handle_streaming_event`.
   Unit-testable; mergeable on its own (no visible change until the chrome
   consumes it).
2. **`ShadedWriter` in `jp_md`.** Lift the background-invariant logic out of
   `TerminalWriter` into a public stateful decorator plus a `shade` convenience,
   and extend `AnsiState` to parse compound SGR parameters.
   Tests cover `\r`/`\x1b[K` rewrites, compound-SGR content backgrounds,
   erase-under-content-background, and resets mid-stream; add characterization
   coverage on the existing markdown code path first.
   Mergeable independently.
3. **OSC-aware tokenization for shaded hyperlinks.** Teach the shared `segments`
   tokenizer (and `ShadedWriter`'s split-escape detection) to recognize OSC
   string sequences (`\x1b]…` terminated by `\x07` or `\x1b\\`) as whole
   escapes, so a tool result's OSC 8 hyperlinks pass through `ShadedWriter`
   verbatim while their visible link text is still shaded.
   Without it the line-oriented tokenizer stops an OSC at its first letter and
   the writer injects the region background into the middle of the URL,
   corrupting the link.
   Tests cover an OSC 8 hyperlink under shading, an OSC split across writes, and
   a `segments`/`visual_width` round-trip on OSC.
   Mergeable independently; a prerequisite for routing results through
   `ShadedWriter` in the next phase.
4. **Per-ID chrome shading in `ToolRenderer`.** Capture the region background by
   tool-call ID at the request boundary; route that tool's writes — the whole
   result included — through a `ShadedWriter` when set.
   `render_code_line` keeps passing `None`; the writer is the sole owner of the
   result's background.
   Depends on phases 1, 2, and 3.
5. **Config field.** Add `style.reasoning.extend_across_tool_calls` (bool,
   default `true`); it lives alongside `style.reasoning.background` and is inert
   when that is unset.
   Gates phase 4.
6. **Replay path.** Capture the per-ID region in `TurnRenderer` and apply the
   same `tool_chrome_visible` predicate as live — this fixes the pre-existing
   bug that replay ignores `style.tool_call.show` and JSON.
   Verify the shaded replay output via snapshot tests.

## References

- [RFD 048] — Four-Channel Output Model (stdout/stderr/tty/log split).
- `crates/jp_cli/src/render/chat.rs` — reasoning background, content-kind
  tracking, deferred separator.
- `crates/jp_cli/src/render/tool.rs` — tool chrome rendering.
- `crates/jp_md/src/shade.rs` — the `ShadedWriter` decorator and `shade`.
- `crates/jp_md/src/writer.rs`, `crates/jp_md/src/ansi.rs` —
  `DefaultBackground` application and `AnsiState` background tracking.
- `crates/jp_config/src/style/reasoning.rs` — reasoning style config.

[RFD 048]: 048-four-channel-output-model.md
[RFD 084]: 084-configurable-markdown-element-coloring.md
