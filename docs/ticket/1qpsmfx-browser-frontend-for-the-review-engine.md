# Browser Frontend for the Review Engine

- **Status**: Todo
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-23
- **Label**: tracking-rfd=140
- **Label**: type=tracking

Tracking ticket for [RFD 140].

## Implementation plan

- [ ] **Add API crate and server extensions**: Extract request and response
      types into `jp-review-api` and add the server additions: diff byte ranges,
      the working-tree flag with `POST /api/reviews/{id}/versions`, `body_html`
      on comments and replies, the `pass` number on stop, and the closing
      message from the host's `answer` on `query_complete` (protocol 16).
      Also adds the HTML shell, `/ui/...` bundle routes, web app manifest, and
      placeholder page.
      Mergeable independently; `/` serves the placeholder until Phase 2.
- [ ] **Build the read-only browser UI**: Create `jp-review-ui` with the review
      list and review detail page in the mobile-first layout (the comments in a
      sheet, and beside the file from 1024 CSS pixels), CSS Custom Highlight
      ranges, version switcher, outdated-comment links, rendered and unified
      diff views, and polling.
      Adds the server address and lock file plus `serve --open`, `start --open`,
      and `open`.
      Also lands the rest of RFD 139's plan (wasm target, `just build-ui` with
      the pinned Tailwind CSS CLI, `wasm_check`/`wasm_test` tool options, `web`
      CI workflow).
      Depends on Phase 1 and RFD 139.
- [ ] **Add interactive commenting and pass controls**: Add selection-to-bytes
      mapping and the actions beside a selection, the comment and suggestion
      composer as a bottom sheet that survives a reload, **Add comment** for
      file-level and review-level comments, picking between cards and passages,
      draft editing and withdrawal, replies and resolution, working-tree
      snapshot recording from the notice, and pass submission and stopping.
      Depends on Phase 2.

[RFD 140]: ../rfd/140-browser-frontend-for-the-review-engine.md
