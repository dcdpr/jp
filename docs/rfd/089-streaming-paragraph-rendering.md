# RFD 089: Streaming Paragraph Rendering

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-28
- **Extends**: [RFD 004]

## Summary

`jp_md::buffer::Buffer` flushes a paragraph only when it sees a block
terminator, so a long single paragraph (common in assistant reasoning) stalls
for up to a minute before anything renders.
This RFD makes the buffer stream a paragraph incrementally as its content
arrives, while guaranteeing the terminal output is **byte-for-byte identical**
to today's whole-paragraph rendering.
The behavior is an opt-out flag on `Buffer`.
Byte-identity has two documented exceptions, neither produced by assistant
output: a setext heading whose content grows past the streaming threshold (it
streams as prose), and a GFM table whose header lacks a leading pipe (it is not
detected and may stream with mis-padded columns).

## Motivation

The streaming render pipeline is `Buffer` (block splitter) → `Formatter`
(per-block comrak parse + word-wrapping `TerminalWriter`) → `Printer`.
`Buffer` emits a paragraph as one `Event::Block` only once it finds a
terminator: a blank line, a setext underline, or an interrupting block.
Until then, every token accumulates.

Assistants routinely emit a long paragraph as **a single line with no internal
newline**: many sentences separated by punctuation, terminated only by the blank
line at the very end.
The whole thing buffers, and the user stares at nothing for as long as the
paragraph takes to generate, observed at over a minute.
Fenced code already streams line-by-line, so the stall is specific to prose.

Two separate things keep that single line from flushing early:

1. **The buffer never classifies it as a paragraph until the line ends.**
   `Buffer` decides what kind of leaf block a line is only once it holds the
   whole line (its first `\n`), because a partial prefix is ambiguous: `#`
   begins a header but `#hello` is a paragraph; a fence, a list marker, and an
   HTML tag are decided the same way.
   For a one-line paragraph that first `\n` arrives only when the paragraph is
   already over, so the block stays unclassified, and therefore unstreamed, the
   whole time.
   This is the dominant stall for the common case.
2. **The setext underline** (`===` / `---`) retroactively turns the preceding
   run of lines into a heading, so once a paragraph is already streaming its
   leading text cannot be committed while a short underline could still follow.
   This one bites only after the block is classified as a paragraph.

Nothing else rewrites already-seen paragraph text at the block level.
Setext content is also multi-line, so its ambiguity covers the entire leading
run, not just one line of look-ahead.

## Design

### The guarantee, and where it lives

The terminal output of a streamed paragraph must equal
`Formatter::format_terminal_with(full_paragraph, opts)` exactly, just emitted in
pieces over time.
Two components share that guarantee, with distinct responsibilities.
The **buffer's partial-line classifier** owns *block-boundary* correctness: it
decides which bytes belong to a paragraph at all, and the renderer cannot repair
a block-level mistake, so it must never enter paragraph mode for a line that
could still become a header, fence, list item, HTML block, reference definition,
or indented code.
The **renderer's hold-in-progress-line rule** then owns *line* stability: given
a correctly-identified paragraph source, it guarantees the streamed pieces
concatenate to `format_terminal_with(full_paragraph, opts)` exactly.
The setext threshold and the inline ground-state scan are neither; they are
latency guards that decide *how much* of an already-identified paragraph the
buffer dares emit, where over-holding costs streaming, not correctness.

The renderer's half holds because `TerminalWriter` is already a
**character-level streaming processor**: its wrapping decisions depend only on
persistent state (`column`, `last_breakable`, `wrap_buffer`), not on how input
is chunked into `output()` calls, and word-wrapping is **greedy and
left-to-right** — once a visual line's break is chosen, no later input revises
it.
That yields the governing invariant:

> The rendered output of `paragraph[..n]` is a byte-prefix of the rendered
> output of `paragraph[..m]` for `n < m`, up to the last committed newline.

The only unstable region is the in-progress final visual line.
So if the renderer emits only committed lines and holds the in-progress line,
and the final emission is exactly today's full render, the total byte stream is
identical to today by construction — regardless of how the buffer chunks the
source or whether the inline scanner is perfectly precise.

### Normal prose does not wait for the whole paragraph

For normal word-wrapped prose with an unambiguous lead, no guard holds the whole
paragraph.
Two documented limitation shapes can still wait until the terminator (see
Drawbacks and Non-Goals): unbreakable runs and ambiguous-lead single-line
paragraphs.
Every hold in the design and its worst-case cost:

| Guard               | What it holds                                                                    | Worst-case latency                                                       |
| ------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| Block-start (entry) | the line's opening token, until block-start is ruled out                         | a few characters for a common lead; up to the line for an ambiguous lead |
| Setext threshold    | the first ~N source bytes, until the paragraph is too long to be a short heading | a fixed prefix (one to two lines), once, at paragraph start              |
| Inline ground-state | an *open* inline construct, from its opener to its closer                        | the width of that construct (a few words)                                |
| Wrap-in-progress    | the current unfinished visual line                                               | one line                                                                 |
| Fixups              | nothing                                                                          | none                                                                     |

The longest anything waits is "the construct currently in flight" or "the line
currently filling."
A 990-word plain-prose paragraph is classified a paragraph at its first
character, holds its first ~N source bytes (setext gate), then streams
continuously to the end; a single `**bold**` mid-paragraph pauses only those ~8
characters while the closing `**` is in flight.
In normal word-wrapped prose nothing accumulates to the whole paragraph.

### Four ambiguities, four guards

Four things can keep a paragraph from streaming, or break prefix-stability once
it does.
Each gets one guard:

| Ambiguity                                                                                                          | Guard                                                                   | Where    |
| ------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------- | -------- |
| **Block-start**: a partial first line may still be a header, fence, list, HTML block, thematic break, or reference | enter paragraph mode only once the prefix rules out every block starter | buffer   |
| **Setext** — `===` reinterprets the run as a heading                                                               | source-byte threshold before emitting any chunk                         | buffer   |
| **Inline span** — a construct opens on one line, closes on a later one                                             | advance the committed prefix only to the last inline ground state       | buffer   |
| **Wrap-in-progress** — the last visual line has not chosen its break                                               | emit only up to the last committed newline; hold the in-progress line   | renderer |

The first three are buffer-side: block-start decides whether to begin streaming
at all, while setext and inline-span decide how much source the buffer dares
hand to the renderer.
The fourth is the renderer-side correctness guarantee.
Each buffer-side guard is conservative in one direction, so its imperfection is
reduced streaming, never wrong output: an unresolved block-start prefix waits
for the newline and is then classified exactly as non-streaming would, while a
too-cautious setext or inline guard commits less and the renderer's
hold-in-progress rule still protects byte-identity.

### Entering paragraph mode from a partial line

The single-line paragraph never reaches the paragraph state machine on its own:
`Buffer` classifies a leaf block only once it has the whole first line, and that
line's terminating `\n` arrives only when the paragraph is already over.
So streaming has to begin from a *partial* first line.

The rule:

> A partial first line is classified as a paragraph the moment its prefix can no
> longer be the prefix of any block starter (ATX header, fenced-code opener,
> thematic break, list marker, HTML block, link-reference definition, or
> indented code).
> Everything after that point is unambiguous paragraph content and streams.

This works in two tiers, split by the line's first non-space character.

**A non-block-starter lead streams immediately.** A letter, or punctuation other
than the block-start characters below, cannot begin any block, so the line is a
paragraph at its first character and streams before the first newline.
This is the common case, and the one this RFD targets.

**A block-starter lead waits for the newline.** `#`, `` ` ``, `~`, `-`, `*`,
`_`, `+`, `<`, `[`, `|`, or a digit leaves the line a viable prefix of a header,
fence, thematic break, list marker, HTML block, link-reference definition,
table, or ordered list.
(`|` leads a GFM table header; a table never streams — see Non-Goals — because
its column widths depend on later rows.)
The buffer does not try to resolve these mid-line; it waits for the first
newline, classifies the line exactly as non-streaming does, and from there
streams the paragraph normally.
`[` is the reason not to bother: a link-reference definition is ruled out only
once `]` is seen without a following `:`, which can be the whole line away, so
there is no short bound to exploit.
For a single-line paragraph led by one of these characters (rare in prose), the
wait extends to the paragraph terminator, a documented limitation (see
Drawbacks), not a heuristic this RFD closes.
A marker line is not a paragraph at all: it enters the existing list path, which
already streams item by item.

**The fallback never misclassifies.** While the prefix is still a viable
block-starter prefix the buffer waits for the `\n` and then classifies exactly
as non-streaming does.
A line that is early-classified is provably a paragraph.
The invariant a test must lock down: *every prefix classified as a paragraph has
only paragraph completions*, so streaming never disagrees with whole-document
parsing about block boundaries; it only changes *when* a paragraph's bytes are
emitted.

Entry is distinct from the setext threshold (below).
Entry answers "is this a paragraph at all"; the threshold answers "has it grown
too long to still become a short heading, so is it safe to emit."
A short `Heading` followed by `===` enters paragraph mode at the `H` but emits
nothing until the threshold, so the `===` still arrives first and forms the
heading.
Both gates are required.

Entry is gated on the streaming toggle: with streaming off, classification keeps
waiting for the full line, so non-streaming output is byte-identical.

### Inline ground-state scan

The inline guard is per-position, not per-paragraph: a paragraph containing
`**bold**` streams everything before the opener and after the closer; only the
open span waits.

A prefix is at **ground state** when every inline construct opened inside it is
also closed inside it.
The scan maintains a stack of unmatched delimiter runs for the extensions
actually enabled (`format.rs:337`): `` ` `` (code), `*` `_` (emphasis /
underline), `~` (strikethrough / subscript), `^` (superscript), `[` / `]` (links
/ images), and `<` (autolinks / raw HTML).
Math is not enabled, so `$` is not tracked.
Ground state ⇔ empty stack.

Two refinements over a naive stack:

- **Links and images need lookahead.** `]` empties the bracket stack, but a
  following `(` or `[` turns the run into a link or reference.
  So `]` at the end of the buffer, or `]` immediately followed by `(` / `[`, is
  treated as *not* ground state until the next character disambiguates.
- **`<` is held** until its construct resolves, since `<...>` may become an
  autolink or raw HTML whose render differs from the literal.

The scan is conservative in one direction only: push on every *potential*
opener, pop only on a confident match.
Flanking subtleties (a literal `*` that is not emphasis) make it *over*-hold —
a little latency — never *under*-hold.

Byte-identity does **not** rest on this scan being perfect — the renderer's
hold-in-progress-line rule is the guarantor.
The scan is a latency heuristic that decides how far the buffer dares commit.
The link refinement matters anyway because [RFD 084] may add ANSI coloring to
links: today a misclassified link survives because links carry no width-shifting
style, but once they do, a too-eager commit would corrupt a committed line.
Tightening the scan now keeps the design robust to that.

### Scope: top-level paragraphs only

`ParagraphChunk` is emitted only from `State::BufferingParagraph` — a paragraph
at the document's top level.
Paragraph-like content *inside* a list item keeps today's behavior:
`handle_in_list` (`buffer.rs:549`) scans and emits it as `Event::Block` segments
item-by-item, and is never routed through the paragraph state machine.
Streaming long prose inside a list item is a separate, larger problem —
preserving list markers, ordered-list renumbering, continuation indent, and
tight/loose spacing — and is out of scope here; the motivating case, long
top-level reasoning prose, is fully covered.
Because chunks only ever come from the top level, `ParagraphChunk.indent` is
always `0`; the field exists for `Event` uniformity.

### Buffer API and the `ParagraphChunk` event

`Buffer` gains a streaming-paragraphs toggle, **on by default**:

```rust
impl Buffer {
    pub fn new() -> Self;                                     // streaming enabled
    pub fn with_streaming_paragraphs(self, on: bool) -> Self; // opt out
}
```

The toggle changes only *timing*, never *appearance* (setext aside) — that
equivalence is the property the crate docs promise and the byte-identity test
locks down.
`jp_md` is in-tree, so the only caller is `ChatRenderer`; there is no
external-compatibility surface beyond the workspace.

While buffering a paragraph past the setext threshold, the buffer emits the
largest inline-safe prefix of new source as:

```rust
Event::ParagraphChunk { content: String, indent: usize, last: bool }
```

Contract:

- `content` is the **source delta** — new paragraph source not previously
  emitted, never cumulative.
- `indent` is always `0`: chunks come only from top-level paragraph buffering
  (see Scope), never from inside a list item.
  The field exists for `Event` uniformity.
- `last` is `true` when the chunk closes the paragraph for rendering: either a
  real paragraph terminator was seen, or `flush_events()` ended the region (see
  Finalization boundaries).
- A non-terminal chunk (`last = false`) never ends inside an open inline
  construct; the terminal chunk (`last = true`) carries whatever source remains
  at the region boundary and may.
- `Display` prints `content` only.

It must be a *distinct* variant: reusing `Event::Block` would make the renderer
separate and independently re-render it, breaking byte-identity.
`Event` gains `#[non_exhaustive]` in the same change so this is the last forced
match-site breakage.
The non-test consumers are `buffer/fixup.rs` and `render/chat.rs`.

### Renderer: render-whole, emit-stable-delta

The renderer keeps the paragraph's accumulated source and a count of bytes
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
ground-state invariants, so the delta slice is always valid.
The cut advances only at committed newlines, which `TerminalWriter` produces
only at word-wrap breaks (it records a breakable point only at a space,
`writer.rs:536`), so a run with no breakable space yields no intermediate cut
and is held until `last` (see the unbreakable-line limitation in Drawbacks).
At `last`, `R` *is* today's full render with today's `opts`, so the
concatenation of all deltas equals `format_terminal_with(full_paragraph, opts)`
byte-for-byte.

Re-parsing the growing buffer each flush is O(n²), but n is one paragraph and
flushes are per-sentence — a few KB of comrak work, negligible.

### Reasoning separator integration

A streamed paragraph is one logical block, so the reasoning-separator
bookkeeping that `print_block` runs once per block runs once per *paragraph*, at
its boundaries — not per chunk:

- Before the first chunk: if the paragraph is reasoning, emit any pending
  reasoning separator shaded (today's `emit_pending_reasoning_separator(true)`,
  `chat.rs:392`).
- Every chunk renders with `suppress_trailing_separator = is_reasoning ||
  !last`.
  Intermediate chunks never emit a trailing separator (it sits past the held
  in-progress line anyway); the final chunk suppresses it only for reasoning,
  exactly as `print_block` does today.
- After the final chunk: if reasoning, set `reasoning_separator_pending = true`
  so `flush()` emits the deferred separator unstyled when the region ends.
- `force_trailing_separator` stays `false`: it only affects tight lists, and a
  paragraph is never one, so the `Block` / `Flush` (mid-stream vs end-of-region)
  distinction does not change a paragraph's bytes.

This keeps the recent separator fixes intact — the shaded gap between
consecutive reasoning blocks, and the unstyled gap when reasoning gives way to a
message or tool call — because the per-chunk `opts` are derived exactly as
`print_block` derives them today.

### Finalization boundaries

A paragraph's streaming state must be closed or discarded at every boundary that
ends a content region, not only at a paragraph terminator:

- **`Buffer::flush_events()`** runs on every content-kind transition (reasoning
  ↔ message ↔ tool call), role header, and end-of-stream.
  For a paragraph mid-stream it emits a terminal `ParagraphChunk { last: true }`
  carrying the remaining buffered source — **not** `Event::Flush`, which the
  renderer would treat as a fresh standalone block.
- **`ChatRenderer::flush()`** drains those events, so the held in-progress line
  is emitted and `para_source` / `emitted` are cleared.
- **`ChatRenderer::reset()`** discards `para_source` / `emitted` along with the
  buffer when an interrupted cycle restarts.

Invariant: after `flush()` returns there is no pending paragraph source, no
pending emitted-byte count, and no held visual line.

### Fixups and streaming

Fixups make LLM output render correctly.
Streaming preserves that fully, because of a structural fact: **no current fixup
rewrites paragraph bytes.** `OrphanedFenceFixup` reads a paragraph only to set a
flag that rewrites the *following* bare fence; `FenceEscalationFixup` touches
only `FencedCode*` events.
The paragraph itself passes through unchanged (`fixup.rs:128`).

So streaming a paragraph and printing it immediately loses no fix.
The only adaptation: `OrphanedFenceFixup` derives its embedded-fence flag from
the streamed chunks instead of an `Event::Block`.
It accumulates the paragraph's source across chunks and computes the flag over
the whole paragraph at the terminal chunk, so the flag is ready exactly when the
following block arrives.
A per-chunk check is not enough: the inline scanner holds an embedded ` ``` `
run intact but commits the prose *before* it in an earlier chunk, so the fence
can land at the *start* of a later chunk, where a line-oriented check mistakes
it for a leading (proper) fence rather than an embedded one.

This generalizes to a constraint on future fixups:

> Streaming a block requires that no fixup retroactively rewrites that block's
> already-emitted bytes based on later content.
> Current fixups satisfy this because they only rewrite *following* fence
> events.
> A future fixup that repairs paragraph prose from whole-paragraph context must
> run in the buffer before streaming, or opt that paragraph out of streaming.

### Setext contract

The setext threshold is measured in **source bytes** the buffer has accumulated
for the current paragraph, deliberately *not* in rendered visual lines.
The `Buffer` is a pure block splitter and does not know the render width
(`buffer.rs:148`); coupling it to `style.markdown.wrap_width` to count visual
lines would break the splitter/renderer boundary for no benefit, since the
threshold gates only timing, never bytes.
The paragraph is *classified* as soon as its prefix rules out every block
starter (see Entering paragraph mode from a partial line); *emission* then waits
until the buffered source exceeds a fixed byte count (~one to two lines' worth),
so a short setext heading is still buffered whole.
Consequences:

- A setext heading short enough to stay under the threshold — every realistic
  one — buffers to its terminator and renders as a heading exactly as today,
  including a single source line that wraps to several visual lines under a
  narrow `wrap_width`.
- A setext heading whose content exceeds the threshold has already begun
  streaming as a paragraph and never becomes a heading.
  This is the byte-difference the threshold introduces; the pipeless-header
  table in Non-Goals is the other documented exception to byte-identity.
- Disabling streaming (`with_streaming_paragraphs(false)`) restores today's
  setext rendering exactly.

The threshold is not user-configurable, so it is an internal tuning constant,
not a public knob.
For non-setext paragraphs byte identity holds regardless of its value; for
setext headings it defines the documented exception boundary, with headings
below it rendering as today and headings above it rendering as paragraphs.

## Drawbacks

- **Over-threshold setext headings render as paragraphs.** A setext heading
  whose content grows past the source-byte threshold streams as prose and never
  becomes a heading.
  This is an accepted, documented byte-difference (the pipeless-header table in
  Non-Goals is the other); realistic (short) headings are unaffected, including
  long single-source-line headings that wrap narrowly.
- **Per-flush re-render is O(n²) per paragraph.** Cheap at paragraph scale, but
  it is real work added to the hot streaming path.
- **`ChatRenderer` gains paragraph-spanning state.** `para_source` / `emitted`
  must be finalized or discarded at every region boundary (`flush`, `reset`,
  content-kind transition), a new failure surface the boundary invariant and
  tests must cover.
- **The inline scan over-holds in exotic cases.** Literal delimiters and
  flanking edge cases delay streaming briefly; this costs latency, not
  correctness.
- **Unbreakable runs and ambiguous-lead paragraphs still stall.** A paragraph
  with no breakable space gives the renderer no wrap commit to cut at, so it
  renders nothing until `last`, however the buffer chunks it.
  A paragraph led by an ambiguous block-start character (notably `[`) does not
  stream before its first source newline; for a single line with no internal
  newline that newline is the terminator, so it never streams.
  The streaming guarantee therefore holds for normal word-wrapped prose with an
  unambiguous lead, not for these.
  Both are rare in assistant output and accepted rather than closed by hard-wrap
  or block-start-threshold heuristics.

## Alternatives

- **Re-render each chunk as an independent block, suppress separators.** Cheap
  and reuses everything, but cannot produce byte-identical output: chunk-local
  wrapping reflows differently and inline constructs spanning a chunk boundary
  render literally.
  Requirement of byte-identity rules it out.
- **Cursor repaint / TUI-style redraw.** Re-render the whole paragraph and
  rewrite earlier lines with cursor movement.
  Fights the printer/typewriter model and the four-channel output model, and
  regresses ANSI-stripped piping (`jp c print | grep`).
  Rejected.
- **Feed one persistent `TerminalWriter` incrementally (no re-parse).** Removes
  the O(n²), but requires making `TerminalFormatter`'s AST walk resumable —
  more machinery for a cost that does not yet matter.
  Deferred as a future optimization.

## Non-Goals

- Adding *new* streaming behavior to non-paragraph blocks.
  Fenced code already streams line-by-line and lists already stream item-by-item
  (`buffer.rs:608`); this RFD changes only paragraph emission and leaves those
  as they are.
  Indented code and HTML blocks keep buffering to their terminator.
- Streaming paragraph-like content inside list items (see Scope); only top-level
  paragraphs stream.
- Preserving setext headings whose content exceeds the streaming threshold.
- Reimplementing CommonMark emphasis flanking for a perfect ground-state
  decision; the conservative scan is sufficient.
- A user-configurable streaming threshold; it is an internal tuning constant.
- Removing the O(n²) re-render; that is a later optimization only if profiling
  demands it.
- Streaming GFM tables.
  A table's rendered column widths depend on its later rows, so its rendering is
  not prefix-stable — committing an early row before a wider one mis-pads it.
  A block whose first line begins with a pipe (every table header in the GFM
  spec, and in assistant output) is kept on the whole-block path and rendered at
  once.
  A pipeless header (`abc | def`) is spec-permitted but appears in no spec
  example and is not produced by assistants; streaming one is a documented
  limitation, not a heuristic this RFD closes.
- Streaming long *unbreakable* text (URLs, hashes, base64, identifiers, or prose
  with no spaces) or ambiguous-lead single-line paragraphs (notably those
  beginning with `[`).
  An unbreakable run has no wrap commit for the renderer to cut at, and an
  ambiguous-lead paragraph does not stream before its first source newline
  (which for a single line is its terminator), so both render to the terminator
  as today.
  Closing them would need hard-wrap or block-start-threshold heuristics whose
  complexity is not justified by their rarity.

## Risks and Open Questions

- **Renderer hold-in-progress correctness** is the load-bearing property.
  Mitigation: a byte-identity harness asserting `streaming == non_streaming`
  over the fixture corpus plus adversarial cases (see Implementation Plan).
  The inline scan is a latency heuristic, not the identity guarantor, so its
  imperfections cost streaming, not correctness.
- **The partial-line paragraph classifier is a new surface.** Unlike the inline
  scan, a misclassification here is a block-level error the renderer cannot
  catch: streaming a header or fence as prose.
  Mitigation: the classifier is conservative (paragraph only when the prefix
  provably cannot begin any block starter), and the byte-identity harness runs
  inputs that lead with every block-starter character to lock the no-divergence
  invariant down.
- **Finalization boundaries** are where held bytes can be silently lost.
  Mitigation: the post-`flush()` invariant above, exercised by tests that
  interleave reasoning, message, and tool-call content mid-paragraph.
- **The test property is new.** `fuzz_buffer.rs` and
  `comrak_cross_validation.rs` assert *event equality* between chunked and whole
  input — a property streaming intentionally breaks — so they must run with
  streaming disabled.
  Byte-identity needs its own harness.
- **Snapshot churn.** The chat-render snapshots encode current event timing;
  chunked output changes the sequence.
  That churn is the visible proof of the behavior change.
- **Threshold tuning.** The setext threshold trades latency against how many
  long setext headings fall into the documented exception.
  Flush cadence is cosmetic for byte output, but the threshold changes the
  exception boundary.

## Implementation Plan

1. **Buffer streaming + partial-line entry + ground-state scan.** Add the
   partial-line paragraph classifier (enter `BufferingParagraph` once the prefix
   rules out every block starter, gated on the streaming toggle), the
   delimiter-stack scan (with link / image lookahead and `<`), the source-byte
   setext threshold, the `with_streaming_paragraphs` toggle (default on), and
   `Event::ParagraphChunk` with `#[non_exhaustive]` on `Event`.
   Existing `buffer_tests.rs`, `fuzz_buffer.rs`, and
   `comrak_cross_validation.rs` run with streaming disabled, since they assert
   event equality; the exhaustive `reassemble` match in
   `comrak_cross_validation.rs` gains a wildcard arm for the new variant.
   Self-contained in `jp_md`.
2. **Renderer + finalization + fixups.** Teach `ChatRenderer` to accumulate
   `ParagraphChunk` source and emit stable deltas with the corrected cut rule,
   finalize at `flush` / `reset` / content-kind transitions, and make
   `OrphanedFenceFixup` derive its embedded-fence flag from chunks on the
   pass-through path.
   Depends on phase 1.
3. **Byte-identity harness.** Assert `streaming == non_streaming` byte-for-byte
   (setext excluded) over the fixture corpus plus adversarial fixtures: no wrap
   before terminator, wrap after several words, inline code / `**strong**` /
   `[label](url)` / image split across chunks, superscript / subscript, orphaned
   fence, reasoning background, and typewriter delay disabled for determinism.
   Add the long-line cases that exercise the entry and cut boundaries: a single
   unbroken token longer than `wrap_width`, a long URL longer than `wrap_width`,
   and paragraphs led by `[`, `<`, and a non-marker digit run.
   Byte identity alone is insufficient for these (a streamed paragraph that
   emits nothing until `last` still passes it), so each fixture also asserts a
   latency shape: word-wrapped prose with an unambiguous lead emits at least one
   chunk before `last`; an unbreakable run emits nothing before `last`; and an
   ambiguous-lead paragraph emits nothing before its first source newline.
   Those assertions lock the accepted limitations in as regression guards.
   Depends on phases 1–2.
4. **Docs.** Document the opt-out toggle, the setext exception, and the
   future-fixup constraint in the `jp_md` crate docs.

## References

- [RFD 004] — the streaming markdown parser/renderer this extends.
- [RFD 084] — configurable markdown coloring; motivates the link-scan
  tightening.
- `crates/jp_md/src/buffer.rs` — `Buffer`, `handle_buffering_paragraph`,
  `Event`.
- `crates/jp_md/src/writer.rs` — `TerminalWriter` greedy word-wrapping.
- `crates/jp_md/src/buffer/fixup.rs` — `OrphanedFenceFixup`,
  `FenceEscalationFixup`.
- `crates/jp_cli/src/render/chat.rs` — `ChatRenderer`, the sole non-test
  consumer of `buffer::Event`.
- `crates/jp_md/tests/fuzz_buffer.rs` — chunked-vs-whole fuzz harness to run
  with streaming disabled.

[RFD 004]: 004-streaming-md-parser-renderer.md
[RFD 084]: 084-configurable-markdown-element-coloring.md
