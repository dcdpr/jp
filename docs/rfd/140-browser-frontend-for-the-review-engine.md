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

The primary target is Safari on iOS.
Desktop browsers run the same application in a wider layout.

- **Layout.** The base layout is a single column containing the rendered file.
  Comments live in a bottom sheet opened from the header or by tapping a
  comment's highlighted passage.
  At `>= 1024px`, the comments panel docks as a sticky column beside the file;
  at `>= 1152px`, a section outline column appears before the file.
  The file column matches the documentation site's width, typography, spacing,
  and color scheme.
  Listing comments in a separate column never inserts vertical gaps into the
  document or alters its line measure.
- **Touch.** On coarse pointers (`pointer: coarse`), buttons and card actions
  measure at least 44x44 CSS pixels (Apple's minimum touch target); on fine
  pointers they use compact sizing.
  No interaction relies on hover.
- **Selection.** iOS renders its native selection callout (Copy, Look Up, Share)
  above a selection, or below a selection near the top of the viewport.
  The review selection popover appears below the selection, positioned clear of
  both the end selection handle and the iOS callout (see
  [Commenting](#commenting)).
- **Keyboard.** On narrow viewports, the composer opens as a bottom sheet sized
  to the visual viewport so the on-screen keyboard never obscures the input.
  On wide viewports, it opens inline in the document under the target passage,
  or directly below the button that triggered it (see
  [Commenting](#commenting)).
- **Suspended tabs.** iOS suspends background tabs and may reload them on
  return.
  The client polls immediately when the tab becomes visible again and persists
  any unsaved composer draft in the tab's `sessionStorage` until it is saved or
  discarded: its kind (new comment, edit, or reply), its target (file, captured
  version, and byte range, or comment number), and its body and replacement
  text.
  Because `sessionStorage` is scoped per tab, two tabs open on the same review
  do not overwrite each other's drafts.
  A restored composer submits against the version it originally captured.
  If the target comment was resolved or withdrawn in the meantime, the API
  rejects the save and leaves the draft text on screen to copy.
- **Home screen.** The HTML shell links a web app manifest (matching `jp serve
  web`) so the UI can be added to the iOS home screen and launched full-screen.

A phone reaches the server using the network options from [RFD 136]: a tunnel
such as `tailscale serve` forwarding to loopback (with the tunnel hostname in
`allowed_hosts`), or `--bind` on a private network.

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

On a phone, the user bookmarks the URL printed by `jp review serve` or adds it
to the home screen.
Both the review list and review detail responses include the workspace name,
which is displayed in the header.
If a response returns a different workspace name than the one the page initially
loaded with (for example, when another workspace's server takes over the same
port), a banner prompts the user to reload.

### Review List

`/` lists all reviews with their title, tracked files, latest version number,
comment counts by status, and active pass indicator, and includes a form for
starting a new review from workspace-relative file paths.

### Review Page

`/reviews/R-1h4k2m9` displays a single review.
On a phone:

```text
+------------------------------+
| jp  Notes   [v3 v] [2] [Send]|
+------------------------------+
| # Notes                      |
|                              |
| The server [binds to every   |
| interface] and has no        |
| authentication.              |
|      [Comment] [Suggest]     |
+------------------------------+
| Comments       [Add] [Close] |
| +--------------------------+ |
| | Jane  open            #1 | |
| | > binds to every inter.. | |
| | This contradicts the     | |
| | Security section.        | |
| |   jp  accept: Fixed.     | |
| +--------------------------+ |
+------------------------------+
```

On a wide screen, the comments sit beside the file:

```text
+--------------------------------------------------------------------+
| jp  Notes         [docs/notes.md v] [Version 3 v] [Send 2 comments]|
+---------------------------------------------+----------------------+
| # Notes                                     | Comments       [Add] |
|                                             |                      |
| The server [binds to every interface] and   | Jane  open        #1 |
| has no authentication.                      | > binds to every in..|
|                                             | This contradicts the |
|                                             | Security section.    |
|                                             |   jp  accept: Fixed. |
|                                             |                      |
|                                             | Jane  draft       #2 |
+---------------------------------------------+----------------------+
```

- **Header.** A single row across all screen sizes containing the review title,
  a file picker (when the review tracks multiple files), a view/version picker
  (selecting a specific version, a pass diff, or a diff between two versions),
  the active pass status, and the primary pass action (**Send**, **Stop**, or
  **Reopen** on a closed review).
  Numeric badges explicitly label what they count (`Version 3`, `Send 2
  comments`).
  Narrow viewports use compact labels and add a speech-bubble toggle button
  showing the comment count to open or close the comments sheet.
- **File pane.** Renders the selected file at the active version (defaulting to
  the latest version) using `md-html`.
  Anchored ranges are highlighted in place with distinct styles for `draft` and
  `open` comments; `resolved` comments and their highlights are hidden until
  toggled on in the comments panel.
- **Comments panel.** Renders one card per comment on the active file:
  review-level, file-level, and outdated comments appear first in creation
  order, followed by range-anchored comments ordered by start offset in the
  file.
  Each card displays the author, status, a truncated quote of the anchored text,
  the rendered comment body, any replacement suggestion (shown as a diff against
  the anchored source text), and the reply thread with assistant verdicts.
  Card actions reflect the comment's state: edit or withdraw a draft comment,
  add or edit a draft reply on an open comment, resolve an open comment, or
  reopen a resolved comment.
- **Selecting comments and passages.** Clicking a comment card highlights its
  passage in the file and scrolls it into view; clicking a highlighted passage
  in the file focuses its card and scrolls the comments panel to it (and opens
  the comments sheet on narrow screens).
  On wide screens, selecting a range-anchored comment also expands its thread
  inline in the document directly beneath the passage.
  Because CSS Custom Highlights are not DOM elements, clicks on the document are
  hit-tested against the client rectangles of highlighted text ranges: the
  shortest matching range is selected first, and subsequent clicks at the same
  point cycle through overlapping comments.
- **Links.** Links inside a rendered file are intercepted rather than resolved
  by the browser against the `/reviews/{id}` URL:
  - A `#fragment` link scrolls to the matching heading anchor.
    The server assigns each Markdown heading the same `id` used by the
    documentation site (`doc::heading`), and opening a URL with a `#fragment`
    scrolls directly to that heading.
  - A relative link to another file tracked by the review switches the file pane
    to that file (and scrolls to its `#fragment` if present).
  - External links (`https:`, `mailto:`) open in a new tab.
  - Relative links to workspace files outside the review are blocked with a
    notice explaining that the target file is not part of the review.
- **Outdated comments.** A comment whose anchored lines changed in a later
  version shows `last seen in version M`, linking to version `M` (using the
  `resolved` anchor projection from the API) with the original range
  highlighted.
- **Working-tree changes notice.** When any tracked file on disk differs from
  the review's latest recorded version and no pass is running, a banner above
  the file offers to snapshot the working tree as a new version and switch the
  view to it.
  While a pass is running, the banner is suppressed because the post-pass
  snapshot records the assistant's edits.

### Commenting

Selecting text in the file pane opens a floating action popover beside the
selection with **Comment** and **Suggest**:

- For mouse selections, the popover is placed near the release position, outside
  the selected text: below the selection's last line after a forward drag, or
  above its first line after a backward drag, horizontally aligned with the
  pointer.
  The popover is measured after rendering before final positioning, using the
  selection's rendered bounding box (which is taller than the glyphs alone).
- While the popover is visible, the browser's native selection marks the range.
  Once the composer opens, the native selection is cleared and replaced with a
  custom highlight so the range is not marked twice.
- For touch-handle or keyboard selections, the popover is placed 30 CSS pixels
  below the selection to clear the end selection handle.
  It also leaves room for the iOS callout when the callout appears below the
  selection, or flips above the selection (and above the callout) when there is
  insufficient space below.
  These offsets match the measurements used by [Plannotator] on iOS.
- Clearing the selection or pressing Escape dismisses the popover.
- The client maps the DOM selection to half-open UTF-8 source byte offsets using
  the `data-s` and `data-e` spans produced by `md-html` ([RFD 124]), snapping
  `data-atom` spans to their boundaries.
  If either endpoint lies inside an `.unmapped` span, the popover displays a
  notice explaining why the selection cannot be anchored instead of the action
  buttons.
- The review ID, file path, version, and byte range are captured when the
  selection is made.
  The suggestion preview and the saved comment use that captured version rather
  than whichever version the pane displays when the comment is saved.
  If the file pane re-renders before the composer opens, the selection is
  cleared and the popover hides.
- **Comment** opens the composer for the Markdown comment body.
  **Suggest** opens it with an editable replacement box, pre-filled with the raw
  source slice covered by the byte range (including any Markdown syntax markers
  such as `**` that do not appear in the rendered text).
- On wide viewports, the composer for a range-anchored item (a new range
  comment, an edit, or a reply) opens inline in the document flow, matching the
  column width, directly below the block where the range ends: after a
  paragraph, heading, table, or code block, or inside a list item below the
  item's own content and before any nested lists (so a comment on an early list
  item does not open at the bottom of a long list).
  Subsequent document content shifts down to make room, and the composer
  receives focus.
  The composer DOM node is reparented into place rather than re-rendered inside
  the Markdown tree, keeping the server-rendered elements intact.
- Saving submits `POST /api/reviews/{id}/comments` with the captured `version`
  and `bytes`.
  Command-Enter or Control-Enter submits from either input field.

Comments without a passage anchor are created from **Add comment** in the
comments panel.
In a single-file review, this creates a review-level comment; in a multi-file
review, the composer prompts whether the comment targets the active file or the
review as a whole.
On wide viewports, the composer for **Add comment** opens directly below that
button, and the pass-message composer opens below **Send**.
While a selection is active or the composer is open, the file pane does not
automatically switch versions.

### Approving and Reading

The selection popover also includes two actions that record the reviewer's
progress through text they have no comment on (as defined in [RFD 136]):

- **Approve** records an approval for the selected passage and highlights it.
  Clicking an approved passage offers to revoke the approval.
  If the passage differs in the displayed version, its highlight is omitted and
  the comments panel lists the approval as changed since it was recorded.
- **Read to here** places a horizontal divider across the file below the block
  where the selection ends, marking the last line the reviewer has read.
  Each file has at most one read-until marker; triggering the action again moves
  it.
  The divider labels the line number (`Read until line 42`), and adds `within
  the block above` when the block extends past that line (for example, when `jp
  review read-until` sets the marker inside a multi-line code block or
  paragraph).

### Passes

- **Send** submits all pending draft comments, draft replies, and unanswered
  reviewer comments with an optional pass message (`POST
  /api/reviews/{id}/passes`), matching `jp review submit` without `--only`.
- While a pass is running, the header displays the pass status, and assistant
  replies and newly opened comments appear as they are written to the store.
  The header shows **Stop** (`POST /api/reviews/{id}/stop`) only when the API
  reports `stoppable = true` (meaning the current server process owns the pass);
  a pass started from the terminal displays `running in another process`.
  **Stop** includes the displayed pass number so a stale tab that missed the end
  of one pass cannot accidentally stop a subsequent pass.
- When the pass finishes, the comments panel displays the pass outcome and the
  assistant's closing message above the comment cards (with earlier passes
  collapsed beneath it), and the file pane switches to the changes view for that
  pass unless the reviewer has an open selection or composer.

The closing message is the final assistant text of the pass's turn.
The host returns it in an optional `answer` field on `query_complete` (plugin
protocol 16), bounded to the same turn range used for structured output.
Because `answer` is optional, `jp-review` retains a minimum protocol version of
15, and passes run under a protocol 15 host simply omit the closing message.
Reading the conversation store after `query_complete` instead would race with
subsequent turns, because the host releases the conversation lock before sending
`query_complete`.
The process running the pass records the closing message on the pass's `Ended`
record so both the browser UI and `jp review show` can display it.
A turn that produces no assistant text records no closing message, and an absent
message never blocks recording the pass outcome or post-pass version.

### Changes View

The changes view renders the target version of a file while highlighting what
changed relative to an earlier version: added and modified lines are highlighted
in the rendered HTML, and deletion markers indicate where lines were removed.
A toggle switches between the rendered changes view and the raw unified diff.

- **Pass changes** compare the version a pass was submitted against with the
  version recorded when it ended, matching `jp review diff --pass N`.
- **Version pickers** allow comparing any two versions `from` and `to`.
- The header's view picker lists each completed pass, and each pass summary in
  the comments panel links to its changes view.
- Clicking a comment card while in the changes view switches back to the file
  view and scrolls to the comment's passage.
- A file deleted in the `to` version is shown as deleted alongside its unified
  diff.
- Changed lines are highlighted via the rendered spans that overlap their byte
  ranges, so a line whose only edit is in Markdown syntax (such as a changed
  link target) still highlights its visible text.
  Changed lines that produce no rendered spans (such as link reference
  definitions or HTML comments) are summarized in a banner above the file (`2
  changed lines not shown`) with a link to the unified diff.

On narrow screens, the unified diff wraps long lines instead of scrolling
horizontally.

### Polling

The client polls `GET /api/reviews/{id}?revision=N` every 2 seconds while idle
and every 500 milliseconds while a pass is running, pauses polling while the tab
is hidden, and polls immediately when the tab becomes visible again.
Every poll response returns `revision`, any active pass status, and whether the
working tree differs from the latest recorded version; the review payload itself
is omitted when `revision` has not advanced past `N`.
Switching versions fetches the review without a `revision` parameter because
comment anchor projections depend on the displayed version, and the client pairs
that response with the corresponding version's HTML.

### Server Changes

This RFD extends the HTTP API from [RFD 136] with these additions:

| Addition                                                                                                                                                                                                                                              | Purpose                                                      |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| Each file in `GET …/diff` includes the byte ranges (in the `to` version) of added or modified lines and the byte offsets where removed lines were deleted.                                                                                            | Highlighting diffs directly in the rendered HTML view.       |
| `GET /api/reviews/{id}` reports on every poll whether tracked working-tree files differ from the latest version (`false` while a pass runs), and `POST /api/reviews/{id}/versions` snapshots them (returning `409 Conflict` while `.pass` is locked). | Driving the working-tree changes banner.                     |
| `GET /api/reviews` and `GET /api/reviews/{id}` include the workspace name.                                                                                                                                                                            | Detecting when a tab remains open across a workspace switch. |
| Comment and reply objects include `body_html`, rendered on the server with `md-html`.                                                                                                                                                                 | Rendering Markdown bodies without bundling `comrak` in Wasm. |
| Each heading in a rendered Markdown file receives the documentation site's heading anchor (`doc::heading`) as its `id`, and the file response returns the list of headings.                                                                           | Supporting in-document `#fragment` links and the outline.    |
| Pass `Ended` records include the assistant's closing message from `query_complete.answer`.                                                                                                                                                            | Displaying the pass summary in the UI and `jp review show`.  |
| `POST /api/reviews/{id}/stop` accepts an optional `pass` number and returns `409 Conflict` without stopping anything if a different pass is running.                                                                                                  | Preventing a stale tab from stopping a newer pass.           |

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

- `/` and `/reviews/{id}` serve the HTML shell (including the workspace name);
  client-side routing switches between the review list and review detail views.
- `/ui/...` serves the embedded WebAssembly binary, the `wasm-bindgen` JS loader
  module, the stylesheet, and the web app manifest with its icon.
- When `jp-review` is compiled without `JP_REVIEW_UI`, `/` and `/reviews/{id}`
  serve the placeholder page described in [RFD 139].

### Security

`jp review serve` is a single-user tool, and the browser UI relies on the server
protections defined in [RFD 136].
Because the UI communicates only with `/api/...` on its own origin, the loopback
binding default, the `allowed_hosts` check, and the same-origin check on
mutating requests cover it without changes, and `md-html` escapes raw HTML in
rendered files and comment bodies.

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
- **Fixed bottom action bar for selections.** A fixed bottom bar behaves
  identically across screen sizes and never collides with the iOS selection
  callout, but on wide screens it places the actions far from the mouse
  selection.
- **Placing the composer only in the comments panel.** Using the comments panel
  for every draft provides a single location, but forces the reviewer to write
  new range comments in a narrow side column away from the passage being
  discussed.
- **Floating the composer over the document text below the selection.** A
  floating popover sits directly under the selected line without shifting
  layout, but obscures the surrounding lines that follow the selection.
- **Rewriting in-document links on the server.** The server knows the review's
  tracked files, but how a link navigates depends on client view state, and the
  client already resolves relative paths against the active file path.
- **Rendering all comment cards inline beneath their blocks (or aligned in a
  wide-screen margin).** Keeping every comment adjacent to its passage avoids
  explicit selection, but sharing a layout row between a document block and a
  comment card stretches the block vertically, and on a phone a long thread
  pushes the surrounding document text off screen.
  Instead, only the active comment expands inline beneath its passage on wide
  screens.
  Google Docs-style margin alignment (positioning cards beside passages without
  stretching the text) requires measuring every passage and card and resolving
  vertical collisions on resize, which can be added to the comments panel later.
- **Multi-row header for file tabs, pass history, and view controls.** Exposes
  more controls simultaneously, but consumed a third of a phone viewport and
  scattered actions across rows with ambiguous scope.

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
- **Tap hit-testing on iOS.** Matching a tap to a highlighted passage relies on
  iOS delivering a `click` event on the document text with client coordinates
  inside the passage's client rectangles; this is verified on an iPhone.
- **Popover placement beside the iOS callout.** The vertical offsets that keep
  the selection popover clear of the iOS callout and selection handles come from
  Plannotator and may need adjustment for this layout.
- **Renderer differences from `jp-vite`.** `md-html` uses `comrak` whereas
  `jp-vite` uses `markdown-it`, so minor rendering differences can occur for
  edge-case Markdown constructs.
- **Testing on iOS.** [RFD 139]'s CI runs browser tests in headless Chrome, and
  WebKit can be driven locally through `safaridriver` on macOS, but iOS-specific
  behaviors (the native selection callout, on-screen keyboard and visual
  viewport, and tab suspension) have no automated test coverage and are verified
  manually on an iPhone before each phase merges.
- **Touch selection precision.** Selecting an exact phrase with touch handles
  may prove too fiddly for frequent use; tapping a block to select it as a whole
  is the fallback if needed.

## Implementation Plan

### Phase 1: API Crate and Server Extensions

Add the workspace member pattern for plugin `api` crates from [RFD 139], and
extract request and response types into `jp-review-api`.
Add diff byte ranges, the working-tree status flag and `POST
/api/reviews/{id}/versions`, `body_html` on comments and replies, the optional
`pass` number on `stop`, and the assistant closing message (`query_complete`
`answer` in plugin protocol 16, persisted on pass outcomes).
Add the HTML shell, `/ui/...` bundle routes, web app manifest, and placeholder
page.
Mergeable independently; `/` serves the placeholder page until Phase 2 lands.

### Phase 2: Read-Only Browser UI

Create `jp-review-ui` with the review list and review detail page in the
mobile-first layout (single column with comments in a bottom sheet below 1024px,
and a side column from 1024px up), CSS Custom Highlight ranges, the version
switcher, outdated-comment links, the rendered and unified diff views, and
polling.
Add the server address and lock file, `serve --open`, `start --open`, and
`open`.
The remainder of [RFD 139]'s plan (`wasm32-unknown-unknown` target, `just
build-ui`, `wasm_check` and `wasm_test` tool options, and the `web` CI workflow)
lands in this phase alongside the first `ui` crate.
Depends on Phase 1 and [RFD 139].

### Phase 3: Interactive Commenting and Pass Controls

Add selection-to-bytes mapping and the floating selection popover; the comment
and suggestion composer (inline in the document on wide screens, bottom sheet on
narrow screens, persisted in `sessionStorage` across reloads); **Add comment**
for file-level and review-level comments; bidirectional selection between cards
and passages; in-document link interception and heading anchors; draft editing
and withdrawal; replies and comment resolution; working-tree snapshot recording
from the banner; and pass submission and stopping.
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
