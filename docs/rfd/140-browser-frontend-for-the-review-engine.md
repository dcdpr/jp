# RFD 140: Browser Frontend for the Review Engine

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-18
- **Requires**: [RFD 139]
- **Extends**: [RFD 136]
- **Summary**: Serves a mobile-first browser UI from `jp review serve` for
  commenting on rendered files and seeing what each pass changed.

## Summary

`jp review serve` serves a browser UI for reviewing workspace files with the
assistant.
The reviewer reads rendered files, selects text to anchor comments or suggest
replacements, submits passes and watches assistant replies arrive in real time,
and inspects per-pass changes highlighted directly in the rendered document.
The UI is designed for Safari on an iPhone first, so a review can continue away
from the desk, and widens to a two-column layout on larger screens.

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

Reviewing prose also needs neither a keyboard nor a terminal.
Reading an RFD on a phone and leaving comments for the next pass turns time away
from the desk into review time, which neither `jp review` nor `revdiff`
supports.

## Design

The frontend is a client-side Leptos application in `jp-review-ui`, built and
embedded into `jp-review` as specified in [RFD 139], sharing request and
response types with the server through `jp-review-api`.

### Mobile First

The primary client is Safari on an iPhone.
A desktop browser runs the same app with a wider layout.

- **Layout.** The base layout is a single column: the file, with each comment
  card inline below the block its anchor starts in, and file-level and
  review-level comments above the file.
  From 1024 CSS pixels wide, cards move into a margin beside their highlights.
- **Touch.** Buttons and card actions are at least 44 by 44 CSS pixels, Apple's
  minimum touch target, and nothing depends on hover.
- **Selection.** iOS shows its own callout (Copy, Look Up, Share) above a
  selection, so the **Comment** action sits in a bar fixed to the bottom of the
  screen (see [Commenting](#commenting)).
- **Keyboard.** The composer opens as a bottom sheet sized to the visual
  viewport, so the on-screen keyboard never covers the text being written.
- **Suspended tabs.** iOS suspends background tabs and may reload them.
  The client polls as soon as the page becomes visible again, and keeps an
  unsaved composer in local storage until it is saved or discarded: its kind (a
  new comment, an edit, or a reply), its target (the file, captured version, and
  bytes, or the comment number), and its body and replacement text.
  A restored composer saves against the version it captured.
  If the target was resolved or withdrawn in the meantime, the API refuses the
  save and the text stays on screen to copy.
- **Home screen.** The shell links a web app manifest, as `jp serve web` does,
  so the UI can be added to the home screen and opened full screen.

The phone reaches the server through options [RFD 136] already provides: a
tunnel such as `tailscale serve` forwarding to the loopback address, with the
tunnel's host name in `allowed_hosts`, or `--bind` on a private network.

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

On a phone, the address `jp review serve` prints is bookmarked or added to the
home screen instead.
The list and review responses carry the name of the workspace the server serves,
and the header shows it from the latest response.
When it differs from the name the page loaded with, as when a tab is left open
while another workspace's server takes over the port, a banner asks to reload.

### Review List

`/` lists all reviews with their title, tracked files, latest version number,
comment counts by status, and active pass indicator, and includes a form for
starting a new review from workspace-relative file paths.

### Review Page

`/reviews/R-1h4k2m9` displays a single review.
On a phone:

```text
+------------------------------+
| jp  Notes             [Stop] |
| Pass 2 running   version 3 v |
| [notes.md]  [api.md]         |
+------------------------------+
| # Notes                      |
|                              |
| The server [binds to every   |
| interface] and has no        |
| authentication.              |
|  +------------------------+  |
|  | #1 Jane, open          |  |
|  | This contradicts the   |  |
|  | Security section.      |  |
|  |   jp, accept: Fixed.   |  |
|  +------------------------+  |
+------------------------------+
|   [ Comment on selection ]   |
+------------------------------+
```

On a wide screen, the cards move into the margin:

```text
+--------------------------------------------------------------------+
| jp  R-1h4k2m9  Notes    version 3 v       Pass 2 running   [Stop]  |
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
- **Comment cards.** One card per comment on the active file, placed as [Mobile
  First](#mobile-first) describes.
  Each card shows the comment's author, status, rendered body, any replacement
  suggestion (diffing the anchored source text against the proposed
  replacement), and the reply thread with assistant verdicts.
  Card actions match the comment's state: edit or withdraw a draft comment, add
  or edit a draft reply on an open comment, or resolve an open comment.
- **Outdated comments.** A comment whose text changed in a later version shows
  `last seen in version M`, linking to version `M` (the `resolved` projection
  the API already returns) with the range highlighted.
- **Working-tree changes banner.** When any tracked file on disk differs from
  the review's latest recorded version and no pass is running, a banner offers
  to snapshot the working tree as a new version and switch the view to it.
  While a pass runs, its post-pass version records the assistant's edits.

### Commenting

Selecting text in the file pane shows a **Comment** bar at the bottom of the
screen:

- The client maps the DOM selection to half-open UTF-8 source byte offsets using
  the `data-s` and `data-e` spans produced by `md-html` ([RFD 124]), snapping
  `data-atom` spans to their boundaries.
  If either endpoint lies inside an `.unmapped` span, the bar explains why the
  selection cannot be anchored instead of offering **Comment**.
- The review, file, version, and byte range are captured together when the
  selection is made.
  The suggestion preview and the saved comment use that captured version, not
  whichever version the pane shows when the comment is saved.
  If the pane re-renders before the composer opens, the selection is dropped and
  the bar hides.
- Tapping **Comment** opens the composer for the Markdown comment body.
  Choosing **Suggest a change** pre-fills an editable replacement box with the
  raw source slice covered by the byte range (including any Markdown syntax
  markers such as `**` that do not appear in the rendered text).
- Saving submits `POST /api/reviews/{id}/comments` with the captured `version`
  and `bytes`.

Buttons in the file tab bar and review header open the composer for file-level
and review-level comments.
While a selection is pending or the composer is open, the file pane does not
switch versions on its own.

### Passes

- **Send** submits all pending draft comments, draft replies, and unanswered
  reviewer comments with an optional pass message (`POST
  /api/reviews/{id}/passes`), matching `jp review submit` without `--only`.
- While a pass is running, the header displays the pass status, and assistant
  replies and newly opened comments appear as they are written to the store.
  The header shows **Stop** (`POST /api/reviews/{id}/stop`) only when the API
  reports the pass `stoppable`, meaning this server runs it; a pass started from
  the terminal reads "running in another process".
  **Stop** names the displayed pass, so a page that missed the end of that pass
  cannot stop the next one.
- When the pass finishes, the header displays the pass outcome and the
  assistant's closing message, and the file pane switches to the changes view
  for that pass (unless the reviewer is actively composing a comment or reply).

The closing message is the last assistant text of the pass's turn.
The host returns it as `answer` on `query_complete` (plugin protocol 16), cut at
the same turn boundary it already uses for structured output.
The field is optional, so `jp-review` keeps requiring protocol 15, and passes
run under an older host record no closing message.
Reading the conversation after `query_complete` instead could find a later
turn's message, because the host releases the conversation before it answers.
The process running the pass stores the message on the pass's `Ended` record, so
both the web UI and `jp review show` display it.
A turn without assistant text records none, and a missing message never prevents
recording the outcome or the final version.

### Changes View

The changes view renders the target version of a file while highlighting what
changed relative to an earlier version: added and modified lines are highlighted
in the rendered HTML, and deletion markers indicate where lines were removed.
A toggle switches between the rendered changes view and the raw unified diff.

- **Pass changes** compare the version a pass was submitted against with the
  version recorded when it ended, matching `jp review diff --pass N`.
- **Version pickers** allow comparing any two versions `from` and `to`.
- The pass history in the header links each completed pass to its changes view.
- A file deleted in the `to` version shows as deleted, with its unified diff.
- Changed lines are highlighted through the rendered text on them, so a line
  whose only change is invisible, such as a link target, still highlights its
  visible text.
  Changed lines with no rendered text at all, such as a reference definition or
  an HTML comment, are counted in the header ("2 changed lines not shown") with
  a link to the unified diff.

On narrow screens, the unified diff wraps long lines instead of scrolling
sideways.

### Polling

The client polls `GET /api/reviews/{id}?revision=N` every 2 seconds while idle
and every 500 milliseconds while a pass is running, stops while the page is
hidden, and polls at once when it becomes visible again.
The response always carries `revision`, the running pass, and whether the
working tree differs from the latest version; it omits the review itself when
`revision` has not moved past `N`.
Switching versions fetches the review without a `revision` cursor, because the
comments' projection depends on the version shown, and the client pairs it with
that version's HTML.

### Server Changes

This RFD extends the HTTP API from [RFD 136] with these additions:

| Addition                                                                                                                                                                                                                                                                                      | Purpose                                                      |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| Each file in `GET …/diff` includes the byte ranges (in the `to` version) of added or modified lines and the byte offsets where removed lines were deleted.                                                                                                                                    | Highlighting diffs directly in the rendered HTML view.       |
| `GET /api/reviews/{id}` reports, beside `revision` and on every poll, whether tracked working-tree files differ from the latest version (false while a pass runs), and `POST /api/reviews/{id}/versions` snapshots them, answering 409 while another process holds the review's `.pass` lock. | Driving the working-tree changes banner.                     |
| `GET /api/reviews` and `GET /api/reviews/{id}` carry the workspace's name.                                                                                                                                                                                                                    | Noticing a tab left open across a change of workspace.       |
| Comment and reply objects include `body_html`, rendered on the server with `md-html`.                                                                                                                                                                                                         | Rendering Markdown bodies without bundling `comrak` in Wasm. |
| Pass `Ended` records include the assistant's closing message, from the host's `answer` on `query_complete`.                                                                                                                                                                                   | Displaying the pass summary in the UI and `jp review show`.  |
| `POST /api/reviews/{id}/stop` takes an optional `pass` number, and answers 409 without stopping anything when another pass is running.                                                                                                                                                        | Keeping a stale page from stopping a newer pass.             |

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

- `/` and `/reviews/{id}` serve the HTML shell, which carries the workspace's
  name; client-side routing switches between the review list and review detail
  views.
- `/ui/...` serves the embedded WebAssembly binary, the `wasm-bindgen` JS loader
  module, the stylesheet, and the web app manifest with its icon.
- When `jp-review` is compiled without `JP_REVIEW_UI`, `/` and `/reviews/{id}`
  serve the placeholder page described in [RFD 139].

### Security

`jp review serve` is a single-user tool, and the UI adds nothing to the
protections [RFD 136] gave the server.
It talks only to `/api/...` on its own origin, so the loopback default, the
`allowed_hosts` check, and the same-origin check on writes cover it unchanged,
and `md-html` escapes raw HTML in files and comment bodies.

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
- **A floating button next to the selection.** Common in desktop review tools,
  but on iOS it competes with the system's selection callout and sits under the
  selection handles; a bar fixed to the bottom of the screen works the same
  everywhere.

## Non-Goals

- Direct file editing or one-click suggestion application in the browser ([RFD
  136]).
- Selecting a subset of draft comments for a pass (`--only`) from the web UI;
  the browser submits all pending items.
- Reviewing git commits or assistant chat messages ([RFD 136]).
- Embedding the review UI inside the `jp-vite` documentation site.
- Authentication, multi-user review sessions, or browser hardening (such as a
  Content-Security-Policy) beyond the server's existing checks.
- Replacing `just review-file` with the browser UI.

## Risks and Open Questions

- **Browser support for CSS Custom Highlight API.** The API is supported in
  Chrome 105+, Safari 17.2+ (including iOS 17.2), and Firefox 140+; older
  browsers show the comment cards without inline text highlights.
- **Margin card collisions.** On wide screens, multiple comments anchored to
  adjacent lines stack vertically and push lower cards below their highlights.
- **Renderer differences from `jp-vite`.** `md-html` uses `comrak` whereas
  `jp-vite` uses `markdown-it`, so minor rendering differences can occur for
  edge-case Markdown constructs.
- **Testing on iOS.** [RFD 139]'s CI runs browser tests in headless Chrome, and
  WebKit can be driven locally through `safaridriver` on macOS, but behavior
  specific to iOS (the selection callout, the keyboard and visual viewport, tab
  suspension) has no automated coverage and is checked on an iPhone before each
  phase merges.
- **Touch selection precision.** Selecting an exact phrase with touch handles
  may prove too fiddly for frequent use; tapping a block to select it whole is
  the fallback if it does.

## Implementation Plan

### Phase 1: API Crate and Server Extensions

Add the workspace member pattern for plugin `api` crates from [RFD 139]'s plan,
and extract request and response types into `jp-review-api`; add diff byte
ranges, the working-tree flag and `POST /api/reviews/{id}/versions`, `body_html`
on comments and replies, the `pass` number on stop, and the closing message (the
host's `answer` on `query_complete`, plugin protocol 16, stored on pass
outcomes); and add the HTML shell, `/ui/...` bundle routes, web app manifest,
and placeholder page.
Mergeable independently; `/` serves the placeholder page until Phase 2 lands.

### Phase 2: Read-Only Browser UI

Create `jp-review-ui` with the review list and the review detail page in the
mobile-first layout (a single column with inline cards, and a margin from 1024
CSS pixels), CSS Custom Highlight ranges, the version switcher, outdated-comment
links, the rendered and unified diff views, and polling.
Add the server's address and lock file, `serve --open`, `start --open`, and
`open`, which need a UI to open.
The rest of [RFD 139]'s plan (the wasm target, `just build-ui`, the `wasm_check`
and `wasm_test` tool options, and the `web` CI workflow) lands here, with the
first `ui` crate.
Depends on Phase 1 and [RFD 139].

### Phase 3: Interactive Commenting and Pass Controls

Add selection-to-bytes mapping and the **Comment** bar, the comment and
suggestion composer as a bottom sheet that survives a reload, file-level and
review-level comment buttons, draft editing and withdrawal, replies and comment
resolution, working-tree snapshot recording from the banner, and pass submission
and stopping.
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
