# RFD 140: Browser Frontend for the Review Engine

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-18
- **Requires**: [RFD 139]
- **Extends**: [RFD 136]
- **Summary**: Serves a browser UI from `jp review serve` for commenting on
  rendered files and seeing what each pass changed.

## Summary

`jp review serve` serves a browser UI for reviewing workspace files with the
assistant.
The reviewer reads rendered files, selects text to anchor comments or suggest
replacements, submits passes and watches assistant replies arrive in real time,
and inspects per-pass changes highlighted directly in the rendered document.

## Motivation

[RFD 136] implemented the review engine, CLI, and JSON HTTP API, while leaving
the browser frontend out of scope.
The HTTP API was designed for a browser client: comment anchors accept UTF-8
byte ranges, `GET /api/reviews/{id}` supports revision polling, and the file
endpoint renders source-mapped HTML via `md-html` ([RFD 124]).

From the terminal, anchoring a comment to a specific phrase requires looking up
and typing 1-based line and column coordinates (e.g. `--at
docs/notes.md:42:5-43:18`) while reading the rendered document in a separate
window.
In a browser, selecting the phrase on the rendered page captures the exact
source range directly.

A second friction point in terminal reviews is inspecting what the assistant
changed during a pass.
`jp review diff --pass last` prints a unified diff of the raw Markdown source,
whereas highlighting added, modified, and removed passages directly inside the
rendered document is much faster to scan when reviewing prose.

## Design

The frontend is a client-side Leptos application in `jp-review-ui`, built and
embedded into `jp-review` as specified in [RFD 139], sharing request and
response types with the server through `jp-review-api`.

### Opening the UI

```sh
jp review serve --open                # start the server, open the review list
jp review start docs/notes.md --open  # start a review and open it
jp review open R-1h4k2m9              # open a review in the running server
```

- `jp review serve` writes its bound address to a file in the review store and
  holds an advisory lock on that file while running.
  Each review store allows at most one active server; a second `jp review serve`
  exits with an error naming the running server's address.
- `jp review open [ID]` opens the review list (or a specific review) in the
  user's default browser.
  If the server lock file is unlocked, `open` reports that no server is running
  and prints the command to start one.
- `jp review start ... --open` creates the review and opens it in the running
  server.

### Review List

`/` lists all reviews with their title, tracked files, latest version number,
comment counts by status, and active pass indicator, and includes a form for
starting a new review from workspace-relative file paths.

### Review Page

`/reviews/R-1h4k2m9` displays a single review:

```text
+--------------------------------------------------------------------+
| R-1h4k2m9  Notes        version 3 v       Pass 2 running   [Stop]  |
| [docs/notes.md]  [docs/api.md]                                     |
+---------------------------------------------+----------------------+
| # Notes                                     | #1 Jane, open        |
|                                             | This contradicts the |
| The server [binds to every interface] and   | Security section.    |
| has no authentication.                      |   jp, accept: Fixed. |
|                                             |                      |
|                                             | #2 Jane, draft       |
|                                             | Suggests: "Anyone... |
+---------------------------------------------+----------------------+
```

- **File pane.** Renders the selected file at the active version (defaulting to
  the latest version) using `md-html`.
  Anchored ranges are highlighted in place using distinct styles for `draft` and
  `open` comments; `resolved` comment highlights are muted and hidden by default
  until toggled on.
- **Margin.** Displays one card per comment on the active file, aligned
  vertically with its highlight; file-level and review-level comments appear at
  the top of the margin.
  On narrow viewports, cards render inline below the block where their anchor
  begins.
- **Comment cards.** Each card shows the comment's author, status, rendered
  body, any replacement suggestion (diffing the anchored source text against the
  proposed replacement), and the reply thread with assistant verdicts.
  Card actions match the comment's state: edit or withdraw a draft comment, add
  or edit a draft reply on an open comment, or resolve an open comment.
- **Outdated comments.** Comments whose lines changed in a later version show an
  `outdated since version N` link that switches the file pane to version `N` and
  scrolls to the highlighted range.
- **Working-tree changes banner.** When any tracked file on disk differs from
  the review's latest recorded version, a banner offers to snapshot the working
  tree as a new version and switch the view to it.

### Commenting

Selecting text in the file pane displays a floating **Comment** button near the
selection:

- The client maps the DOM selection to half-open UTF-8 source byte offsets using
  the `data-s` and `data-e` spans produced by `md-html` ([RFD 124]), snapping
  `data-atom` spans to their boundaries.
  If either endpoint lies inside an `.unmapped` span, the button is disabled and
  explains why the selection cannot be anchored.
- Clicking **Comment** opens a composer for the Markdown comment body.
  Choosing **Suggest a change** pre-fills an editable replacement box with the
  raw source slice covered by the byte range (including any Markdown syntax
  markers such as `**` that do not appear in the rendered text).
- Saving submits `POST /api/reviews/{id}/comments` with `bytes` and the
  currently displayed `version`.

Buttons in the file tab bar and review header open the composer for file-level
and review-level comments.

### Passes

- **Send** submits all pending draft comments, draft replies, and unanswered
  reviewer comments with an optional pass message (`POST
  /api/reviews/{id}/passes`), matching `jp review submit` without `--only`.
- While a pass is running, the header displays the pass status and a **Stop**
  button (`POST /api/reviews/{id}/stop`), and assistant replies and newly opened
  comments appear in the margin as they are written to the store.
- When the pass finishes, the header displays the pass outcome and the
  assistant's closing message, and the file pane switches to the changes view
  for that pass (unless the reviewer is actively composing a comment or reply).

The closing message is the final assistant chat response from the pass's turn.
When `query_complete` arrives, the process running the pass reads the
conversation events from the host, extracts the last assistant response, and
persists it on the pass's `Ended` record in `review.json` so both the web UI and
`jp review show` can display it.

### Changes View

The changes view renders the target version of a file while highlighting what
changed relative to an earlier version: added and modified lines are highlighted
in the rendered HTML, and deletion markers indicate where lines were removed.
A toggle switches between the rendered changes view and the raw unified diff.

- **Pass changes** compare the version a pass was submitted against with the
  version recorded when it ended, matching `jp review diff --pass N`.
- **Version pickers** allow comparing any two versions `from` and `to`.
- The pass history in the header links each completed pass to its changes view.

### Polling

The client polls `GET /api/reviews/{id}?revision=N` every 2 seconds while idle
and every 500 milliseconds while a pass is running, pausing polling whenever the
browser tab is hidden.

### Server Changes

This RFD extends the HTTP API from [RFD 136] with four additions:

| Addition                                                                                                                                                   | Purpose                                                      |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| Each file in `GET …/diff` includes the byte ranges (in the `to` version) of added or modified lines and the byte offsets where removed lines were deleted. | Highlighting diffs directly in the rendered HTML view.       |
| `GET /api/reviews/{id}` reports whether tracked working-tree files differ from the latest version, and `POST /api/reviews/{id}/versions` snapshots them.   | Driving the working-tree changes banner.                     |
| Comment and reply objects include `body_html`, rendered on the server with `md-html`.                                                                      | Rendering Markdown bodies without bundling `comrak` in Wasm. |
| Pass `Ended` records include the assistant's closing message from the turn.                                                                                | Displaying the pass summary in the UI and `jp review show`.  |

Example diff entry:

```json
{
  "path": "docs/notes.md",
  "change": "modified",
  "diff": "@@ -40,3 +40,3 @@\n...",
  "added": [[4113, 4148]],
  "removed": [3980]
}
```

All request and response types move into `jp-review-api` without breaking
existing JSON fields.

Static UI routes served alongside `/api/...`:

- `/` and `/reviews/{id}` serve the HTML shell; client-side routing switches
  between the review list and review detail views.
- `/ui/...` serves the embedded WebAssembly binary, the `wasm-bindgen` JS loader
  module, and the stylesheet.
- When `jp-review` is compiled without `JP_REVIEW_UI`, `/` and `/reviews/{id}`
  serve the placeholder page described in [RFD 139].

### Security

- The UI communicates exclusively with `/api/...` on the same origin and is
  subject to the server's existing loopback default, `allowed_hosts` check, and
  same-origin enforcement on state-changing requests.
- The HTML shell sets a `Content-Security-Policy` header that restricts scripts
  and styles to `'self'`, permits the single inline module loader by SHA-256
  hash and WebAssembly compilation via `'wasm-unsafe-eval'`, and blocks all
  external origins.
- File contents and comment bodies are rendered on the server by `md-html`,
  which escapes raw HTML and strips `javascript:`, `vbscript:`, `file:`, and
  non-image `data:` URLs.

## Drawbacks

- **Polling latency and traffic.** Assistant replies appear with up to 500 ms of
  polling delay, and an open tab polls every 2 seconds while idle.
- **Explicit version snapshotting.** The file pane displays recorded versions
  rather than live disk contents; external editor saves must be snapshotted via
  the banner before commenting on new lines.
- **Build step required for the UI.** Building `jp-review` with plain `cargo
  install` instead of `just install-plugin review` serves the placeholder page.
- **Additional workspace crates.** `jp-review` expands from one crate to three
  (`jp-review`, `jp-review-api`, and `jp-review-ui`).

## Alternatives

- **Server-sent events (SSE) instead of polling.** Because `review_reply` and
  `review_comment` run in separate short-lived tool processes that write
  directly to `review.json`, pushing updates over SSE would require filesystem
  watching in the server; polling `revision` reuses the existing stateless
  endpoint and can be revisited if polling overhead becomes measurable.
- **Auto-snapshotting on every file save.** Automatically recording a new
  version whenever a file changes on disk would create noisy intermediate
  versions on every editor save and prematurely outdate comments on lines being
  edited.
- **Client-side Markdown rendering.** Compiling `comrak` into the WebAssembly
  bundle would increase bundle size to duplicate rendering that the server
  already performs with `md-html`.
- **Source-only diff view.** Relying solely on unified source diffs duplicates
  `jp review diff` without making prose changes easier to read in context.

## Non-Goals

- Direct file editing or one-click suggestion application in the browser ([RFD
  136]).
- Selecting a subset of draft comments for a pass (`--only`) from the web UI;
  the browser submits all pending items.
- Reviewing git commits or assistant chat messages ([RFD 136]).
- Embedding the review UI inside the `jp-vite` documentation site.
- Authentication or multi-user review sessions.
- Replacing `just review-file` with the browser UI.

## Risks and Open Questions

- **Browser support for CSS Custom Highlight API.** The API is supported in
  Chrome 105+, Safari 17.2+, and Firefox 140+; older browsers will render
  comment cards in the margin without inline text highlights.
- **Margin card collisions.** Multiple comments anchored to adjacent lines will
  stack vertically and push lower cards below their corresponding text
  highlights.
- **Renderer differences from `jp-vite`.** `md-html` uses `comrak` whereas
  `jp-vite` uses `markdown-it`, so minor rendering differences can occur for
  edge-case Markdown constructs.
- **Touch selection behavior.** Mapping selections on mobile and tablet browsers
  relies on native touch selection handles and has not been tested.

## Implementation Plan

### Phase 1: API Crate and Server Extensions

Extract request and response types into `jp-review-api`; add diff byte ranges,
working-tree dirty detection and `POST /api/reviews/{id}/versions`, `body_html`
on comments and replies, and the assistant's closing message on pass outcomes;
add the HTML shell, `/ui/...` bundle routes, placeholder page, and
`Content-Security-Policy`; and implement the server lock/address file, `serve
--open`, `start --open`, and `open`.
Mergeable independently (serves the placeholder page until Phase 2 lands).

### Phase 2: Read-Only Browser UI

Create `jp-review-ui` with the review list view, the review detail page (file
pane, CSS Custom Highlight ranges, margin cards, version switcher, and outdated
comment links), the rendered and unified diff views, and revision polling.
Depends on Phase 1 and [RFD 139].

### Phase 3: Interactive Commenting and Pass Controls

Add selection-to-bytes mapping and the floating **Comment** button, the comment
and suggestion composer, file-level and review-level comment buttons, draft
editing and withdrawal, replies and comment resolution, working-tree snapshot
recording from the banner, and pass submission and stopping.
Depends on Phase 2.

## References

- [RFD 124: Source-Mapped Markdown Rendering][RFD 124]
- [RFD 136: Review Engine for Workspace Files][RFD 136]
- [RFD 139: Browser UIs in Rust with Leptos][RFD 139]
- [Plannotator]

[Plannotator]: https://plannotator.ai/
[RFD 124]: 124-source-mapped-markdown-rendering.md
[RFD 136]: 136-review-engine-for-workspace-files.md
[RFD 139]: 139-browser-uis-in-rust-with-leptos.md
