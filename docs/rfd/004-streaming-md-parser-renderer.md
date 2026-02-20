# RFD 004: Streaming Markdown Parser and Terminal Renderer in `jp_md`

- **Status**: Implemented
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-19

## Summary

`jp_md` contains two custom components alongside `comrak`: a block-boundary
detector (`Buffer`) for streaming segmentation, and a terminal renderer for
escape-sequence-aware styled output. This ADR records why we maintain both and
why we chose comrak over pulldown-cmark.

## Context

JP streams LLM responses token-by-token. To render markdown progressively, we
need to know where one block ends and the next begins — before the full response
has arrived. Neither comrak nor pulldown-cmark supports incremental parsing;
both require the complete input upfront.

A CommonMark document is a sequence of block-level elements, and a single block
is a valid document. So if we can segment the stream into individual blocks, any
standard parser can handle each one independently. `Buffer` does this
segmentation: it buffers incoming chunks and drains them as soon as it
recognizes a complete block. It implements just enough of the CommonMark
block-level grammar (headings, fenced/indented code, HTML blocks, paragraph
interruption rules) to detect boundaries — inline parsing is left entirely to
comrak.

Fenced code blocks are the exception to one-block-at-a-time buffering: Buffer
streams the opening fence, each content line, and the closing fence as separate
events. This allows line-by-line syntax highlighting without waiting for the
full block. The trade-off is that an unclosed fence causes all subsequent
content to render as code. Buffering the full block would be strictly worse: the
user waits for the entire remaining response only for the parser to discover the
fence was never closed.

Once a block is segmented, it needs to be rendered as styled terminal output.
The initial approach injected `Node::Raw` ANSI escape nodes into comrak's AST
([comrak#743]), then used comrak's CommonMark formatter for output. This broke
in two ways: comrak's line wrapping counts escape sequences as visible
characters, producing incorrect wrap points; and when a wrap happened between an
opening and closing escape sequence, the unclosed sequence caused incorrect
rendering. The current terminal renderer is ANSI-aware — it counts only visible
characters for wrapping, and closes open sequences before a line break and
re-opens them after.

Since we no longer manipulate comrak's AST, switching to pulldown-cmark is
technically possible. However, comrak still offers a built-in CommonMark
formatter (`format_commonmark`) for non-terminal output and a convenient AST.
With pulldown-cmark we'd need a separate crate (`pulldown-cmark-to-cmark`) for
the same formatting capability.

[comrak#743]: https://github.com/kivikakk/comrak/pull/743

## Decision

We maintain two custom components in `jp_md`:

1. **`Buffer`** — a block-boundary detector for streaming segmentation.
2. **`TerminalRenderer`** — an ANSI-aware AST walker for styled terminal output
   with correct line wrapping and sequence management.

We use comrak for parsing (AST construction) and non-terminal formatting. We
choose comrak over pulldown-cmark for its built-in CommonMark formatter and AST
convenience.

Quality goals, in priority order: minimally buffered (time-to-first-token),
streaming (tokens/second), well-formatted (themed terminal output), correct
(spec-faithful). Correctness is last deliberately — we prefer showing content
with potentially wrong styling over making the user wait.

## Consequences

- Buffer and comrak must agree on block boundaries. If they diverge, the
  affected block renders incorrectly and damage may cascade until the parser
  state self-corrects. The existing proptest fuzz suite verifies Buffer's
  self-consistency (chunked vs. whole-document), but does not yet cross-validate
  against comrak's block boundaries — that is a potential next step.
- We maintain ~400 lines of block-level grammar that partially overlaps with
  comrak's parser. This is the cost of streaming capability no existing crate
  provides.
- An unclosed fenced code block misrenders all subsequent content as code. This
  is an accepted latency-over-correctness trade-off.

## References

- `crates/jp_md/src/buffer.rs` — block-boundary detector
- `crates/jp_md/src/format.rs` — comrak-based formatter
- `crates/jp_md/src/render.rs` — terminal renderer (comrak AST walker)
- [comrak](https://github.com/kivikakk/comrak)
- [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark)
