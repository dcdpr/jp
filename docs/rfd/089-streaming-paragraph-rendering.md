# RFD 089: Streaming Paragraph Rendering

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-28
- **Extends**: [RFD 004]
- **Summary**: Streams markdown paragraphs incrementally in `jp_md::Buffer`
  while preserving byte-identical output with whole-paragraph rendering.

## Summary

This RFD updates `jp_md::buffer::Buffer` to stream markdown paragraphs
incrementally as tokens arrive instead of waiting for a block terminator.
Four look-ahead guards (block-start classification, setext underline buffering,
open inline spans, and word-wrap stability) keep the streamed terminal output
byte-for-byte identical to whole-paragraph rendering, with documented exceptions
only for oversized setext headings and pipeless GFM tables.

## Motivation

The streaming render pipeline is `Buffer` (block splitter) → `Formatter`
(per-block comrak parse and word-wrapping `TerminalWriter`) → `Printer`.
`Buffer` emits a paragraph as a single `Event::Block` only after finding a
terminator: a blank line, a setext underline, or an interrupting block.
Until then, every token is buffered.

Assistants routinely emit a long paragraph as **a single line with no internal
newline**, with many sentences separated by punctuation and terminated only by a
trailing blank line.
The entire paragraph buffers, leaving the terminal blank while the paragraph
generates (observed at over a minute).
Fenced code blocks already stream line-by-line, so the stall is specific to
prose.

Two ambiguities prevent a single-line paragraph from flushing early:

1. **Line-end block classification.** `Buffer` classifies a leaf block only
   after receiving the full line (up to its first `\n`), because a partial
   prefix can be ambiguous: `#` begins an ATX heading whereas `#hello` is a
   paragraph, and fences, list markers, and HTML tags have similar prefix rules.
   In a single-line paragraph, the first `\n` arrives only at the end of the
   paragraph, so the block remains unclassified and unstreamed throughout
   generation.
   This is the primary cause of the stall.
2. **Setext underlines.** A trailing `===` or `---` line retroactively turns the
   preceding lines into a setext heading.
   Even after a block is classified as a paragraph, its leading text cannot be
   committed while a short underline could still follow.

No other CommonMark construct retroactively rewrites paragraph text at the block
level.
Because setext heading content can span multiple lines, its ambiguity covers the
entire leading run rather than a single line of look-ahead.

## Design

### Byte-Identity Guarantee

The terminal output of a streamed paragraph must equal
`Formatter::format_terminal_with(full_paragraph, opts)` byte-for-byte, emitted
incrementally over time.

Incremental emission is possible because `TerminalWriter` is already a
**character-level streaming processor**: its wrapping decisions depend only on
persistent state (`column`, `last_breakable`, `wrap_buffer`), not on how input
is chunked into `output()` calls.
Because word-wrapping is **greedy and left-to-right**, once a visual line
chooses its break point, later input never revises it.
This gives the governing invariant:

> The rendered output of `paragraph[..n]` is a byte-prefix of the rendered
> output of `paragraph[..m]` for `n < m`, up to the last committed newline.

Only the in-progress final visual line is unstable.
When the renderer emits only committed lines, holds the in-progress line, and
performs the full render on the final chunk, the concatenated byte stream equals
whole-paragraph rendering by construction.

### Four Ambiguities and Guards

Four ambiguities can prevent a paragraph from streaming or break prefix
stability once streaming begins:

| Ambiguity                                                                                                          | Guard                                                                   | Where    |
| ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------- | -------- |
| **Block-start**: a partial first line may still be a header, fence, list, HTML block, thematic break, or reference | enter paragraph mode only once the prefix rules out every block starter | buffer   |
| **Setext**: `===` or `---` reinterprets the preceding run as a heading                                             | buffer a source-byte threshold before emitting any chunk                | buffer   |
| **Inline span**: a construct opens on one line and closes on a later one                                           | advance the committed prefix only to the last inline ground state       | buffer   |
| **Wrap-in-progress**: the last visual line has not chosen its break                                                | emit only up to the last committed newline; hold the in-progress line   | renderer |

Two of these guards enforce correctness, while the other two trade latency for
earlier streaming:

- The **partial-line classifier** (`Buffer`) owns *block-boundary* correctness.
  It decides whether bytes belong to a paragraph at all.
  Because the renderer cannot repair a block-level misclassification, the buffer
  never enters paragraph mode for a line that could still become a header,
  fence, list item, HTML block, reference definition, or indented code block.
- The **hold-in-progress-line rule** (`ChatRenderer`) owns *line* stability.
  Given a valid paragraph source, it guarantees that the streamed chunks
  concatenate to `format_terminal_with(full_paragraph, opts)` regardless of how
  the buffer chunks the source or whether the inline scanner is exact.
- The **setext threshold** and **inline ground-state scan** (`Buffer`) are
  latency guards that decide how much of an identified paragraph the buffer
  emits early.
  Each buffer-side guard is conservative in one direction, so any imprecision
  reduces streaming rather than producing wrong output: an unresolved
  block-start prefix waits for the newline and classifies as in non-streaming
  mode, while a conservative setext or inline hold defers bytes to the next
  chunk while the renderer's hold-in-progress rule preserves byte identity.

### Latency Bounds for Normal Prose

Each guard holds at most the construct or line currently in flight:

| Guard               | What it holds                                                                    | Worst-case latency                                                       |
| ------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| Block-start (entry) | the line's opening token, until block-start is ruled out                         | a few characters for a common lead; up to the line for an ambiguous lead |
| Setext threshold    | the first ~N source bytes, until the paragraph is too long to be a short heading | a fixed prefix (one to two lines), once, at paragraph start              |
| Inline ground-state | an *open* inline construct, from its opener to its closer                        | the width of that construct (a few words)                                |
| Wrap-in-progress    | the current unfinished visual line                                               | one line                                                                 |
| Fixups              | nothing                                                                          | none                                                                     |

For normal word-wrapped prose with an unambiguous lead, no guard buffers the
entire paragraph.
A 990-word plain-prose paragraph is classified as a paragraph at its first
character, buffers its first ~N source bytes for the setext gate, and then
streams continuously to the end; a `**bold**` span mid-paragraph pauses only
those ~8 characters while the closing `**` is in flight.
Only two rare shapes still wait for the block terminator: unbreakable runs with
no spaces and single-line paragraphs with an ambiguous block-starter lead (see
Drawbacks and Non-Goals).

### Entering Paragraph Mode from a Partial Line

Because a single-line paragraph has no `\n` until its terminator, `Buffer` must
begin streaming from a partial first line:

> A partial first line is classified as a paragraph the moment its prefix can no
> longer be the prefix of any block starter (ATX header, fenced-code opener,
> thematic break, list marker, HTML block, link-reference definition, or
> indented code).
> Everything after that point is unambiguous paragraph content and streams.

Classification splits into two tiers based on the line's first non-space
character:

- **Non-block-starter leads stream immediately.** A letter, or punctuation other
  than the block-starter characters below, cannot begin any other block.
  The line is classified as a paragraph at its first character and streams
  before the first newline.
- **Block-starter leads wait for the newline.** A leading `#`, `` ` ``, `~`,
  `-`, `*`, `_`, `+`, `<`, `[`, `|`, or digit leaves the line a viable prefix of
  an ATX header, code fence, thematic break, list marker, HTML block,
  link-reference definition, GFM table, or ordered list.
  (`|` starts a GFM table header; tables never stream because column widths
  depend on later rows.)
  Rather than resolving these prefixes mid-line, the buffer waits for the first
  `\n`, classifies the full line as in non-streaming mode, and streams any
  resulting paragraph from that point on.
  `[` motivates this cutoff: a link-reference definition is ruled out only once
  `]` is seen without a following `:`, which can be an entire line away.
  For a single-line paragraph starting with one of these characters, waiting for
  `\n` means waiting for the paragraph terminator (see Drawbacks).
  List marker lines enter the existing list path, which already streams item by
  item.

Because ambiguous prefixes wait for `\n`, early classification never
misclassifies: *every prefix classified as a paragraph has only paragraph
completions*.
Streaming never disagrees with whole-document parsing on block boundaries; it
changes only when a paragraph's bytes are emitted.

Block-start entry is distinct from the setext threshold.
Entry determines whether the block is a paragraph; the setext threshold
determines whether the paragraph has grown too long to become a short setext
heading.
A short `Heading` followed by `===` enters paragraph mode at `H` but emits
nothing below the threshold, so `===` arrives before any output and forms the
heading.

Partial-line entry is gated on the streaming toggle: when streaming is disabled,
`Buffer` waits for the full line before classifying.

### Inline Ground-State Scan

The inline guard operates per position rather than per paragraph: in a paragraph
containing `**bold**`, everything before the opening `**` and after the closing
`**` streams, and only the open span waits.

A prefix is at **ground state** when every inline construct opened inside it is
also closed inside it.
The scanner maintains a stack of unmatched delimiter runs for the enabled comrak
extensions (`format.rs:337`): `` ` `` (code), `*` and `_` (emphasis and
underline), `~` (strikethrough and subscript), `^` (superscript), `[` and `]`
(links and images), and `<` (autolinks and raw HTML).
Math is disabled, so `$` is not tracked.
The prefix is at ground state when this stack is empty.

Two constructs require look-ahead beyond simple delimiter matching:

- **Links and images.** Closing `]` empties the bracket stack, but a following
  `(` or `[` turns the span into an inline or reference link.
  A `]` at the end of the buffer, or `]` immediately followed by `(` or `[`, is
  treated as non-ground until a subsequent character rules out a link tail.
- **Autolinks and raw HTML.** `<` is held until its construct resolves, because
  `<...>` may parse as an autolink or raw HTML whose rendered output differs
  from the literal characters.

The scan is conservative in one direction: it pushes on every potential opener
and pops only on a definite match.
CommonMark flanking edge cases (such as a literal `*` that does not open
emphasis) cause the scanner to hold extra bytes briefly, trading latency for
safety.

Although the renderer's hold-in-progress-line rule already protects byte
identity for unstyled links, the link look-ahead is necessary because [RFD 084]
may add ANSI coloring to links.
Once links carry styling or alter rendered width, committing a partial link
across a line wrap would corrupt an already emitted line.

### Scope: Top-Level Paragraphs Only

`ParagraphChunk` is emitted only from `State::BufferingParagraph`, which handles
paragraphs at the document's top level.
Paragraph content inside a list item continues through `handle_in_list`
(`buffer.rs:549`), which scans and emits `Event::Block` segments item by item
without routing through the paragraph state machine.
Streaming long prose inside list items requires preserving list markers,
ordered-list renumbering, continuation indentation, and tight or loose spacing;
that is out of scope for this RFD, which targets top-level reasoning and message
prose.

### Buffer API and `Event::ParagraphChunk`

`Buffer` gains a paragraph-streaming toggle, **enabled by default**:

```rust
impl Buffer {
    pub fn new() -> Self;                                     // streaming enabled
    pub fn with_streaming_paragraphs(self, on: bool) -> Self; // opt out
}
```

Aside from oversized setext headings and pipeless tables, the toggle changes
only emission timing, never rendered output.
Because `jp_md` is an in-tree crate whose only non-test caller is
`ChatRenderer`, there is no external compatibility surface outside the
workspace.

While buffering a paragraph past the setext threshold, `Buffer` emits the
longest ground-state prefix of unemitted source as:

```rust
Event::ParagraphChunk { content: String, indent: usize, last: bool }
```

Contract:

- `content` is the **source delta**: new paragraph source not previously
  emitted, never cumulative.
- `indent` is always `0` because chunks come only from top-level paragraph
  buffering, never from inside a list item.
  The field is kept for `Event` uniformity.
- `last` is `true` when the chunk closes the paragraph for rendering, either at
  a paragraph terminator or when `flush_events()` ends the content region (see
  Finalization Boundaries).
- A non-terminal chunk (`last = false`) never ends inside an open inline
  construct; the terminal chunk (`last = true`) carries whatever source remains
  at the region boundary, even if an inline construct is unclosed.
- `Display` prints `content` only.

`ParagraphChunk` must be a distinct variant from `Event::Block`, which would
cause the renderer to format each chunk as a separate block with block
separators.
`Event` gains `#[non_exhaustive]` in the same change to avoid future exhaustive
match breakage; the non-test consumers are `buffer/fixup.rs` and
`render/chat.rs`.

### Renderer: Render Whole, Emit Stable Delta

The renderer accumulates the paragraph's source and tracks the byte offset
already printed:

```text
on ParagraphChunk { content, last }:
    if first chunk of this paragraph and is_reasoning:
        emit_pending_reasoning_separator(shaded = true)   // as print_block does today
    para_source.push_str(content)
    opts = terminal_options(indent)                       // today's per-block opts
    opts.suppress_trailing_separator = is_reasoning || !last
    opts.force_trailing_separator    = false              // paragraphs are never tight lists
    R = format_terminal_with(&para_source, opts)
    cut = if last {
        R.len()
    } else {
        // Hold the in-progress visual line. format_terminal_with always
        // finalizes with a trailing newline (TerminalWriter::finish), so the
        // last line of a non-final render is never a real wrap commit. Drop
        // that synthetic newline, then cut at the last *committed* newline.
        match R.trim_end_matches('\n').rfind('\n') {
            Some(i) => i + 1,
            None    => 0,   // nothing committed yet; hold everything
        }
    }
    printer.print(&R[emitted..cut])                       // typewriter delay as today
    emitted = cut
    if last:
        if is_reasoning: reasoning_separator_pending = true
        reset paragraph state
```

`R[..emitted]` is unchanged across re-renders by the greedy-wrap and
ground-state invariants, so slicing `&R[emitted..cut]` is always valid.
The cut offset advances only at committed newlines, which `TerminalWriter`
produces only at word-wrap breaks (recording breakable points only at spaces in
`writer.rs:536`).
A run without a breakable space produces no intermediate cut and is held until
`last = true` (see Drawbacks).
On the final chunk (`last = true`), `R` is the full paragraph render, so the
concatenated deltas equal `format_terminal_with(full_paragraph, opts)`
byte-for-byte.

Re-parsing the growing paragraph on each flush is O(n²) in paragraph length, but
`n` is a single paragraph (a few kilobytes of comrak work across sentence-sized
flushes), which is negligible.

### Reasoning Separator Integration

A streamed paragraph is one logical block, so the reasoning-separator
bookkeeping that `print_block` runs once per block runs once per paragraph at
its boundaries rather than per chunk:

- **Before the first chunk**: if the paragraph is reasoning content, call
  `emit_pending_reasoning_separator(true)` (`chat.rs:392`) to emit any pending
  reasoning separator with background shading.
- **On every chunk**: render with `suppress_trailing_separator = is_reasoning ||
  !last`.
  Intermediate chunks never emit a trailing separator (which sits past the held
  in-progress line anyway), and the final chunk suppresses it only for
  reasoning, matching `print_block`.
- **After the final chunk**: if the paragraph is reasoning content, set
  `reasoning_separator_pending = true` so `flush()` emits the deferred separator
  unstyled when the region ends.
- **`force_trailing_separator` remains `false`**: this flag affects only tight
  lists, so the `Block` versus `Flush` (mid-stream versus end-of-region)
  distinction does not alter a paragraph's rendered bytes.

Deriving per-chunk `opts` the same way `print_block` does preserves both the
shaded separator between consecutive reasoning blocks and the unstyled separator
when reasoning transitions to a message or tool call.

### Finalization Boundaries

A paragraph's streaming state must be finalized or discarded at every boundary
that ends a content region, not only at a paragraph terminator:

- **`Buffer::flush_events()`** runs on every content-kind transition (reasoning
  ↔ message ↔ tool call), role header, and end-of-stream.
  For a paragraph mid-stream, it emits a terminal `ParagraphChunk { last: true
  }` carrying the remaining buffered source instead of `Event::Flush`, which the
  renderer would format as a separate standalone block.
- **`ChatRenderer::flush()`** drains those events, emits the held in-progress
  line, and clears `para_source` and `emitted`.
- **`ChatRenderer::reset()`** discards `para_source` and `emitted` along with
  the buffer when an interrupted cycle restarts.

Invariant: after `flush()` returns, there is no pending paragraph source, no
pending emitted-byte count, and no held visual line.

### Fixups and Streaming

Streaming preserves LLM output fixups because **no existing fixup modifies
paragraph bytes**.
`OrphanedFenceFixup` inspects a paragraph only to set a flag that rewrites the
*following* bare fence, and `FenceEscalationFixup` operates only on
`FencedCode*` events.
Paragraph content passes through unchanged (`fixup.rs:128`).

To work with streamed paragraphs, `OrphanedFenceFixup` accumulates paragraph
source across `ParagraphChunk` events and computes its embedded-fence flag over
the full paragraph at the terminal chunk (`last = true`), before the following
block arrives.
A per-chunk check is insufficient: the inline scanner holds an embedded ` ``` `
run intact while committing the preceding prose in an earlier chunk, which can
place the fence at the start of a later chunk where a line-oriented check would
mistake it for a valid opening fence rather than an embedded one.

This establishes a general constraint for future fixups:

> Streaming a block requires that no fixup retroactively rewrites that block's
> already-emitted bytes based on later content.
> A future fixup that repairs paragraph prose from whole-paragraph context must
> either run in the buffer before streaming or opt that paragraph out of
> streaming.

### Setext Contract

The setext threshold is a fixed internal constant measured in accumulated
**source bytes**, not rendered visual lines.
`Buffer` is a pure block splitter and does not know the render width
(`buffer.rs:148`); coupling it to `style.markdown.wrap_width` to count visual
lines would break the splitter-renderer boundary for a threshold that only gates
emission timing.

Once a partial line is classified as a paragraph (see Entering Paragraph Mode
from a Partial Line), emission waits until the buffered source exceeds the byte
threshold (roughly one to two lines of text):

- **Below the threshold**: a setext heading buffers to its terminator and
  renders as a heading, including a single source line that wraps across several
  visual lines under a narrow `wrap_width`.
- **Above the threshold**: a setext heading begins streaming as a paragraph
  before its underline arrives and never becomes a heading.
  Along with pipeless GFM tables (see Non-Goals), this is the only exception to
  byte-identical output.
- **Streaming disabled**: `with_streaming_paragraphs(false)` buffers all
  paragraphs to their terminator and renders setext headings of any length.

## Drawbacks

- **Over-threshold setext headings render as paragraphs.** A setext heading
  whose body exceeds the source-byte threshold streams as prose and never
  becomes a heading.
  Short setext headings are unaffected, including single-source-line headings
  that wrap narrowly.
- **Per-flush re-rendering is O(n²) per paragraph.** Re-parsing and formatting
  the growing paragraph on each flush is quadratic in paragraph length, though
  cheap at single-paragraph scale.
- **`ChatRenderer` holds paragraph-spanning state.** `para_source` and `emitted`
  must be finalized or cleared at every region boundary (`flush`, `reset`, and
  content-kind transitions), adding state management that boundary tests must
  cover.
- **The inline scanner over-holds on edge cases.** Literal delimiters and
  CommonMark flanking edge cases can delay streaming briefly, costing latency
  rather than correctness.
- **Unbreakable runs and ambiguous-lead single-line paragraphs still buffer to
  the terminator.** A paragraph with no breakable space gives the renderer no
  word-wrap newline to cut at, so nothing is emitted until `last = true`.
  A paragraph starting with an ambiguous block-starter character (notably `[`)
  does not stream before its first source newline, which for a single-line
  paragraph is its terminator.
  Both shapes are rare in assistant output and do not justify hard-wrap or
  mid-line block-starter heuristics.

## Alternatives

- **Render each chunk as an independent block with suppressed separators.**
  Formatting chunks independently reuses the existing block path, but
  chunk-local word wrapping reflows differently and inline constructs spanning a
  chunk boundary render literally, violating byte identity.
- **Cursor repaint (TUI-style redraw).** Re-rendering the paragraph and
  rewriting earlier lines with terminal cursor movement conflicts with the
  printer/typewriter model and the four-channel output model, and breaks
  ANSI-stripped piping (`jp c print | grep`).
- **Feed a persistent `TerminalWriter` incrementally without re-parsing.**
  Avoiding per-chunk re-parsing removes the O(n²) cost, but requires making
  `TerminalFormatter`'s AST walk resumable.
  Deferred unless profiling shows per-paragraph re-parsing is a bottleneck.

## Non-Goals

- **Adding new streaming behavior to non-paragraph blocks.** Fenced code already
  streams line-by-line and lists stream item-by-item (`buffer.rs:608`); indented
  code and HTML blocks continue to buffer to their terminator.
- **Streaming paragraph content inside list items** (see Scope: Top-Level
  Paragraphs Only).
- **Preserving setext headings whose content exceeds the streaming threshold.**
- **Reimplementing CommonMark emphasis-flanking rules for exact ground-state
  detection.** The conservative delimiter scan is sufficient.
- **Exposing a user-configurable streaming threshold.**
- **Eliminating the O(n²) per-paragraph re-render.**
- **Streaming GFM tables.** A table's rendered column widths depend on later
  rows, so table rendering is not prefix-stable: committing an early row before
  a wider one mis-pads the columns.
  A block whose first line starts with `|` (covering every table header in the
  GFM spec and in assistant output) stays on the whole-block path.
  A pipeless table header (`abc | def`) is permitted by the GFM spec but does
  not appear in spec examples or assistant output; streaming it as a paragraph
  is a documented exception.
- **Streaming long unbreakable text (URLs, hashes, base64, identifiers, or
  spaceless prose) or ambiguous-lead single-line paragraphs (such as those
  beginning with `[`).** Both buffer until their terminator (see Drawbacks).

## Risks and Open Questions

- **Hold-in-progress line slicing in `ChatRenderer`.** Byte identity depends on
  never emitting bytes from the unfinished final visual line before `last =
  true`.
  Mitigation: a byte-identity harness asserting `streaming == non_streaming`
  across the fixture corpus and adversarial cases (see Implementation Plan).
- **Partial-line paragraph classification.** Unlike the inline scan (which only
  affects latency), a misclassification here is a block-level error the renderer
  cannot repair, such as streaming a header or code fence as prose.
  Mitigation: the classifier enters paragraph mode only when the prefix cannot
  begin any block starter, and the test suite exercises inputs starting with
  every block-starter character.
- **Finalization boundaries.** Held bytes can be silently lost if a content
  region ends without flushing paragraph state.
  Mitigation: enforce the post-`flush()` invariant in tests that interleave
  reasoning, message, and tool-call content mid-paragraph.
- **Event-equality test suites.** `fuzz_buffer.rs` and
  `comrak_cross_validation.rs` assert event equality between chunked and whole
  input, which paragraph streaming intentionally breaks.
  Mitigation: run those suites with streaming disabled and verify rendered
  output in a dedicated byte-identity harness.
- **Snapshot churn.** Chat-render snapshots encode event timing and will change
  when paragraphs emit in chunks.
- **Threshold tuning.** The setext byte threshold trades initial streaming
  latency against how many long setext headings fall into the documented
  exception.

## Implementation Plan

1. **Buffer streaming, partial-line entry, and ground-state scan.** Add the
   partial-line paragraph classifier (entering `BufferingParagraph` once the
   prefix rules out every block starter, gated on the streaming toggle), the
   delimiter-stack scan (with link/image look-ahead and `<`), the source-byte
   setext threshold, the `with_streaming_paragraphs` toggle (enabled by
   default), and `Event::ParagraphChunk` with `#[non_exhaustive]` on `Event`.
   Run `buffer_tests.rs`, `fuzz_buffer.rs`, and `comrak_cross_validation.rs`
   with streaming disabled because they assert event equality, and add a
   wildcard arm to the exhaustive `reassemble` match in
   `comrak_cross_validation.rs`.
   Self-contained in `jp_md`.
2. **Renderer, finalization, and fixups.** Update `ChatRenderer` to accumulate
   `ParagraphChunk` source, emit committed-line deltas, and finalize state at
   `flush`, `reset`, and content-kind transitions.
   Update `OrphanedFenceFixup` to derive its embedded-fence flag from
   accumulated chunks on the terminal chunk.
   Depends on phase 1.
3. **Byte-identity harness.** Assert `streaming == non_streaming` byte-for-byte
   (excluding oversized setext headings) over the fixture corpus and adversarial
   fixtures: no wrap before terminator, wrap after several words, inline code /
   `**strong**` / `[label](url)` / image split across chunks, superscript /
   subscript, orphaned fence, reasoning background, and typewriter delay
   disabled for determinism.
   Add long-line boundary fixtures: a single unbroken token longer than
   `wrap_width`, a URL longer than `wrap_width`, and paragraphs starting with
   `[`, `<`, and a non-marker digit run.
   Because a paragraph that emits nothing until `last` still satisfies byte
   identity, each fixture also asserts emission timing: word-wrapped prose with
   an unambiguous lead emits at least one chunk before `last`; an unbreakable
   run emits nothing before `last`; and an ambiguous-lead paragraph emits
   nothing before its first source newline.
   Depends on phases 1 and 2.
4. **Docs.** Document `with_streaming_paragraphs`, the setext exception, and the
   future-fixup constraint in the `jp_md` crate docs.

## References

- [RFD 004]: the streaming markdown parser and renderer this RFD extends.
- [RFD 084]: configurable markdown coloring, which motivates the link look-ahead
  in the ground-state scan.
- `crates/jp_md/src/buffer.rs`: `Buffer`, `handle_buffering_paragraph`, and
  `Event`.
- `crates/jp_md/src/writer.rs`: `TerminalWriter` greedy word-wrapping.
- `crates/jp_md/src/buffer/fixup.rs`: `OrphanedFenceFixup` and
  `FenceEscalationFixup`.
- `crates/jp_cli/src/render/chat.rs`: `ChatRenderer`, the sole non-test consumer
  of `buffer::Event`.
- `crates/jp_md/tests/fuzz_buffer.rs`: chunked-versus-whole fuzz harness run
  with streaming disabled.

[RFD 004]: 004-streaming-markdown-parser-and-terminal-renderer-in-jp_md.md
[RFD 084]: 084-configurable-markdown-element-coloring.md
