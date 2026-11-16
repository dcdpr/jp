# RFD 004: Streaming Markdown Parser and Terminal Renderer in jp_md

- **Status**: Implemented
- **Category**: Decision
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-19
- **Extended by**: [RFD 084], [RFD 089]
- **Summary**: Pairs `comrak` with a custom block-boundary `Buffer` and
  ANSI-aware `TerminalRenderer` for progressive markdown rendering in `jp_md`.

## Summary

`jp_md` pairs `comrak` with two custom components: a block-boundary detector
(`Buffer`) for streaming segmentation and an ANSI-aware terminal renderer
(`TerminalRenderer`) for styled output.
This RFD records why JP maintains both components and why it uses `comrak`
rather than `pulldown-cmark`.

## Context

JP streams LLM responses token by token.
Progressive markdown rendering requires detecting block boundaries before the
full response arrives, but neither `comrak` nor `pulldown-cmark` supports
incremental parsing; both require the complete input upfront.

Because a CommonMark document is a sequence of block-level elements and each
block is a valid document on its own, a standard parser can parse each block
independently once the stream is segmented.
`Buffer` performs this segmentation by buffering incoming chunks and draining
each block as soon as its boundary is reached.
It implements only enough of the CommonMark block-level grammar (headings,
fenced and indented code blocks, HTML blocks, and paragraph interruption rules)
to detect block boundaries, leaving inline parsing to `comrak`.

Fenced code blocks are the exception to whole-block buffering: `Buffer` emits
the opening fence, each content line, and the closing fence as separate events
so the renderer can syntax-highlight code line by line without waiting for the
full block.
The trade-off is that an unclosed fence causes all subsequent content to render
as code.
Buffering the entire code block to avoid this would be worse: the user would
wait for the rest of the response only for the parser to discover the fence was
never closed.

Segmented blocks then render as styled terminal output.
The initial approach injected `Node::Raw` ANSI escape nodes into `comrak`'s AST
([comrak#743]) and used `comrak`'s CommonMark formatter for output.
This failed in two ways: `comrak`'s line wrapper counted escape sequences as
visible characters, wrapping lines too early, and line breaks between opening
and closing escape sequences left sequences unclosed across lines.
`TerminalRenderer` walks the AST directly instead: it counts only visible
characters when wrapping, closes active ANSI sequences before a line break, and
reopens them on the next line.

Because `TerminalRenderer` does not modify `comrak`'s AST, switching to
`pulldown-cmark` is technically possible.
However, `comrak` provides a convenient AST and a built-in CommonMark formatter
(`format_commonmark`) for non-terminal output, whereas `pulldown-cmark` would
require a separate crate (`pulldown-cmark-to-cmark`).

## Decision

`jp_md` maintains two custom components:

1. **`Buffer`**: a block-boundary detector for streaming segmentation.
2. **`TerminalRenderer`**: an ANSI-aware AST walker for styled terminal output
   with visible-width line wrapping and escape-sequence management.

`jp_md` uses `comrak` rather than `pulldown-cmark` for AST construction and
non-terminal CommonMark formatting because `comrak` provides both a convenient
AST and a built-in formatter.

`jp_md` orders its quality goals by priority: minimal buffering
(time-to-first-token), streaming throughput (tokens/second), well-formatted
output (themed terminal rendering), and spec-faithful correctness.
Correctness comes last by design: rendering content immediately with potentially
wrong styling is preferable to making the user wait.

## Consequences

- `Buffer` and `comrak` must agree on block boundaries.
  If they diverge, the affected block renders incorrectly and errors can cascade
  until the parser state self-corrects.
  The `proptest` fuzz suite verifies `Buffer`'s self-consistency (chunked vs.
  whole-document input), but does not yet cross-validate against `comrak`'s
  block boundaries.
- `jp_md` maintains ~400 lines of block-level grammar that partially overlaps
  with `comrak`'s parser, which is the cost of streaming support that no
  existing crate provides.
- An unclosed fenced code block renders all subsequent content as code, an
  accepted latency-over-correctness trade-off.

## References

- `crates/jp_md/src/buffer.rs`: block-boundary detector
- `crates/jp_md/src/format.rs`: `comrak`-based formatter
- `crates/jp_md/src/render.rs`: terminal renderer (`comrak` AST walker)
- [comrak]
- [pulldown-cmark]

[RFD 084]: 084-configurable-markdown-element-coloring.md
[RFD 089]: 089-streaming-paragraph-rendering.md
[comrak]: https://github.com/kivikakk/comrak
[comrak#743]: https://github.com/kivikakk/comrak/pull/743
[pulldown-cmark]: https://github.com/pulldown-cmark/pulldown-cmark
