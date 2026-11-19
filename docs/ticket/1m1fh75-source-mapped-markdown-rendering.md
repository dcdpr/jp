# Source-Mapped Markdown Rendering

- **Status**: Done
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-16
- **Label**: tracking-rfd=124
- **Label**: type=tracking

Tracking ticket for [RFD 124].

## Implementation plan

- [x] **Create the md-html crate**: Adds `crates/internal/md-html` with
      `render`, byte-offset to `line:column` conversion helpers, and unit tests
      for each span type and each range adjustment.
      Includes a corpus test that renders every Markdown file under `docs/rfd`,
      `docs/wiki`, and `docs/ticket` and asserts no text node or code span is
      emitted as `unmapped`.
      The phase is self-contained and mergeable before any consumer is wired up.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-17T09:39:57Z

RFD 124 is implemented, so this ticket closes.
The open item, removing the `qual:api` markers from `md_html::Lines`, moved to
T-1md6ywd: it waits on the first code that calls those methods, expected in D72.

[RFD 124]: ../rfd/124-source-mapped-markdown-rendering.md
