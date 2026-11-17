# RFD 124: Source-Mapped Markdown Rendering

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-16
- **Summary**: Adds an internal markdown-to-HTML renderer whose rendered text
  carries the byte range of the source it came from.

## Summary

This RFD adds `md-html`, an internal crate that renders Markdown to HTML with
source byte ranges attached to every rendered text span.
A page displaying that HTML can map a text selection directly to an exact source
byte range and map a stored source range back to a highlight without searching
for text.

## Motivation

Any tool that lets a reader annotate rendered Markdown (to comment on a phrase,
suggest a replacement, or link to a passage) must identify the exact source text
the selection covers.
Searching the Markdown source for the selected text is unreliable:

- Duplicate phrases are ambiguous.
- Rendered text differs from source text: `**Abandoned**` renders as
  "Abandoned", and `\|` in a table cell renders as `|`.
- A paragraph that appears on one line in the browser may span several lines in
  the source, so a selection crosses line endings that are invisible on the
  page.

JP renders Markdown to HTML in two places, and neither preserves source
positions:

- **The documentation site** renders with markdown-it via VitePress. markdown-it
  tokens carry line ranges only for block tokens.
  Its maintainer closed a request for column positions as "impossible with
  current architecture" ([markdown-it issue 821][mdit-821]), and its paragraph
  rule passes a rebuilt, trimmed string to the inline parser, so inline offsets
  do not correspond to source offsets.
- **`jp-serve-web`** renders conversation messages with comrak's
  `markdown_to_html`.

comrak records a source position on every AST node, and six workspace crates
already use it to parse Markdown.
What is missing is an HTML renderer that emits those positions in a form a web
page can consume, along with verification that the positions are exact.

## Design

### What a Caller Gets

```rust
let rendered = md_html::render(source);
rendered.html // String
```

```markdown
This RFD is **Abandoned**.
```

```html
<p><span data-s="327" data-e="339">This RFD is </span><strong><span data-s="341" data-e="350">Abandoned</span></strong><span data-s="352" data-e="353">.</span></p>
```

Every contiguous run of rendered text is wrapped in a `<span>` carrying `data-s`
and `data-e`, the half-open byte range (`[start, end)`) in `source` that
produced it.
Markdown syntax characters (`**`, backticks, link destinations, list markers)
produce no rendered text and appear in no span.
Whitespace inserted by the renderer between spans (line endings, or the space
that replaces a line break inside a wrapped code span) is also outside any span;
a cursor position there lies in a gap.

The renderer emits three kinds of spans:

| Span                            | Meaning                                                                                                             |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| `data-s`, `data-e`              | The rendered text matches the source bytes character for character. An offset into the text maps 1:1 to the source. |
| `data-s`, `data-e`, `data-atom` | The rendered text represents the source range as an indivisible unit. Selections snap to its boundaries.            |
| `class="unmapped"`              | The source range could not be verified. The text cannot be selected as an annotation endpoint.                      |

Atoms are used in two places: the `|` rendered from an escaped pipe (`\|`) in a
table cell, and an alert's title, which comrak synthesizes from the alert kind
to represent the `[!NOTE]` marker.

The renderer never guesses: if a node's source range does not resolve back to
the exact rendered text, the renderer marks the span `unmapped`.

Positions are UTF-8 byte offsets because browser code can compute them directly
from a DOM text node with `TextEncoder`.
The crate also converts between byte offsets and 1-based `line:column` positions
(with columns counted in Unicode code points) for callers that persist
line-and-column anchors.

### Client-Side Mapping

`md-html` ships no JavaScript; the client-side mapping contract has two
operations:

- **Selection to source range.** For each endpoint of the DOM selection, locate
  its enclosing span.
  The source byte offset is `data-s` plus the UTF-8 byte length of the span text
  preceding the selection offset.
  Inside a `data-atom` span, a selection start snaps to `data-s` and a selection
  end snaps to `data-e`.
  If an endpoint falls in a gap outside any span (such as between two
  paragraphs), a selection start snaps forward to the next span's `data-s` and a
  selection end snaps backward to the previous span's `data-e`.
  If either endpoint falls inside an `unmapped` span, the selection has no
  source range and cannot be annotated.
  A selection whose endpoints are both mapped may span across `unmapped` text,
  because the byte range between the two endpoints still covers the underlying
  source.
- **Source range to highlight.** Every span whose `[data-s, data-e)` interval
  overlaps the target byte range is highlighted over the intersecting character
  subrange; `data-atom` spans are highlighted in full.

This mapping was prototyped in a browser across three RFDs using the CSS Custom
Highlight API (which highlights DOM ranges without mutating the HTML), covering
single-line selections, selections across emphasis, code spans, links, and
paragraphs, selections in table cells and code blocks, and persistence across
page reloads.

### Deriving Exact Ranges

`md-html` parses Markdown with comrak (enabling tables, strikethrough,
autolinks, task lists, and alerts) and walks the AST to emit HTML directly.
A text node's byte range comes from its comrak `sourcepos`, with three
adjustments:

- **`parse.escaped_char_spans` is enabled.** Without this option, a text node
  containing a backslash escape reports a `sourcepos` that includes the
  backslash, shifting character offsets after the escape.
- **Code spans are split by source line.** A code span's `sourcepos` includes
  its delimiting backticks, and a multi-line code span collapses each line
  ending (along with the next line's indentation or blockquote prefix) into a
  single rendered space.
  The renderer strips the backticks and emits a separate mapped `<span>` for
  each source line, separated by an unmapped space.
- **Escaped pipes in table cells are compensated.** Inside a table cell, comrak
  counts an escaped pipe (`\|`) as one column instead of two bytes, shifting
  subsequent column positions on the same line left by one byte per `\|`.
  The renderer scans from the start of the table cell and adds one byte for each
  preceding `\|`.

Fenced code blocks are mapped line by line: each content line corresponds to the
trailing slice of its source line, after any indentation or blockquote prefix.

### Corpus Verification

Measured across 403 Markdown files in this repository (`docs/rfd`,
`docs/rfd/drafts`, `docs/wiki`, and `docs/ticket`) with comrak 0.52:

| Node              |   Count | Resolves back to exact source                                      |
| ----------------- | ------: | ------------------------------------------------------------------ |
| Text              | 115,404 | 100%. 8 fail without `escaped_char_spans`.                         |
| Code span         |  41,942 | 100%. 632 wrap across lines; 5 contain an escaped pipe in a table. |
| Fenced code block |   1,634 | 100%, line by line.                                                |

By container, the text nodes include 44,402 in list items, 4,647 in table cells,
6,442 in headings, and 526 in alerts.

The same pass found two inaccuracies in block-level `sourcepos` ranges, which
`md-html` does not rely on because it maps leaf text and code nodes directly:

- 1,163 of 11,078 list items report their end position as column 0 of the
  following line.
- One table nested inside a list item reports its end position two columns
  short.

### Scope Boundaries

- **Raw HTML is escaped** and rendered as literal text.
  Callers render untrusted or semi-trusted Markdown inside pages with access to
  local APIs.
- **Image `alt` text** is written into an HTML attribute and receives no span.
- **JP-specific syntax.** Mention autolinks (such as `T-02wt0kx`) and embeds
  belong to `doc` and `jp-wiki`, not `md-html`.
  `md-html` reports byte offsets into the exact string passed to `render`.
  A caller that transforms the Markdown source before rendering (for example, by
  expanding embeds) is responsible for mapping offsets back to the original
  file; a caller that post-processes the HTML may wrap spans in outer elements
  as long as it preserves their text and attributes.
- **Syntax highlighting** in code blocks is omitted.

## Drawbacks

- **A second HTML renderer alongside VitePress.** Documents rendered by
  `md-html` do not share VitePress's exact HTML structure or plugins, so any
  Markdown extension added to the documentation site must either be ported to
  `md-html` or differ between the two views.
- **Coupling to comrak's column accounting.** A comrak upgrade that changes how
  column offsets are calculated could invalidate one of the range adjustments.
  The corpus test over JP's documentation catches regressions at test time.
- **Larger HTML output.** Wrapping every text node and code line in a `<span>`
  increases HTML size compared to plain comrak output.

## Alternatives

### comrak's `render.sourcepos`

comrak has a built-in `render.sourcepos` option that adds `data-sourcepos`
attributes to HTML elements.
However, it annotates block and inline elements rather than text nodes: plain
text between child elements has no wrapper element of its own, element ranges
include surrounding Markdown delimiters, and the caller would still need to
apply the backslash, multi-line code span, and table-pipe adjustments on the
client.

### Patch markdown-it

Adding byte-accurate inline source spans to markdown-it would let VitePress and
review views share one renderer.
Doing so requires wrapping every inline rule and mapping each block rule's
rebuilt, trimmed input string back to the original file offsets: a change the
markdown-it maintainer rejected as incompatible with its architecture, inside a
VitePress release line that is still in alpha.
Because stored anchors are source positions rather than DOM positions, nothing
in this RFD precludes switching renderers later.

### pulldown-cmark

`pulldown-cmark`'s offset iterator yields a byte range for every event,
including text events.
We did not adopt it because six workspace crates already parse Markdown with
comrak; introducing a second Markdown parser would risk subtle parsing
divergences where `md-html` and `doc` or `jp-wiki` disagree on document
structure.

### Text search in the source

Locating selections by searching the raw Markdown source requires no renderer
changes, but fails on duplicate phrases, inline formatting syntax, and wrapped
lines as described in [Motivation](#motivation).

## Non-Goals

- Replacing the VitePress documentation renderer.
- Migrating `jp-serve-web` to `md-html`.
- Shipping client-side selection or highlighting JavaScript in `md-html`.

## Risks and Open Questions

- **Markdown constructs absent from JP's corpus.** JP's documents contain no
  inline HTML and no HTML entity or character references outside code spans, and
  neither footnotes nor description lists are enabled (comrak notes that
  description list source positions are unreliable).
  When a construct's source range does not match its rendered text (for example,
  an HTML entity like `&amp;` whose 5-byte source renders as `&`), the node
  falls back to `class="unmapped"`, failing closed rather than producing a wrong
  offset.
- **Tab characters.** JP's Markdown files indent with spaces; how comrak's
  column counter advances across literal `\t` characters has not been verified.

## Implementation Plan

### Phase 1: `md-html` crate

Create `crates/internal/md-html` with `render`, byte-to-`line:column` conversion
helpers, and unit tests covering each span type and each range adjustment.

Include a corpus test that renders every Markdown file under `docs/rfd`,
`docs/wiki`, and `docs/ticket` and asserts that no text node or code span is
emitted as `unmapped`, catching comrak regressions or newly introduced edge
cases in CI.

This phase is self-contained and mergeable before any consumer is wired up.

## References

- [markdown-it issue 821][mdit-821], on column positions
- [comrak's `render.sourcepos`][comrak-sourcepos]

[comrak-sourcepos]: https://docs.rs/comrak/0.52.0/comrak/options/struct.Render.html#structfield.sourcepos
[mdit-821]: https://github.com/markdown-it/markdown-it/issues/821
