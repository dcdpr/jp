# Review Engine for Workspace Files

- **Status**: Done
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-17
- **Label**: tracking-rfd=136
- **Label**: type=tracking

Tracking ticket for [RFD 136].

## Implementation plan

- [x] **Build core model and store**: Implement the review domain model, anchor
      projection across versions, content-addressed blob storage, locked
      `review.json` persistence, and the non-turn CLI commands (`start`, `ls`,
      `show`, `diff`, `comment`, `reply`, `edit`, `withdraw`, `resolve`,
      `close`).
      Extract the advisory file lock helper from `ticket::board` into `doc`, and
      convert positions with `md_html::Lines`, removing the `qual:api` marker
      from each method as the core first calls it.
      Runs no turns and is mergeable on its own.
- [x] **Add passes and review tools**: Implement pass prompt formatting,
      `submit`, the `review_reply` and `review_comment` plugin tools, `tools
      init`, and `init` (`review.toml` generation).
      Passes hold the `.pass` owner lock from the start, and a pass left without
      an outcome is recorded as `interrupted`.
      Depends on Phase 1; afterwards the full review loop works end to end from
      the terminal.
- [x] **Extract shared plugin protocol client**: Move the async plugin protocol
      client into `jp_plugin::client` behind a `client` Cargo feature, and
      migrate `jp-serve-web` and `jp-serve-chat` to it with no behavioral
      change.
      Mergeable independently of Phases 1 and 2.
- [x] **Implement `jp review serve`**: Implement the HTTP API, pass stopping,
      and graceful shutdown.
      Add plain-text rendering to `md-html` for files other than Markdown.
      Depends on Phases 2 and 3.

[RFD 136]: ../rfd/136-review-engine-for-workspace-files.md
