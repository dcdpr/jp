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

- **Layout.** The base layout is a single column holding the file.
  The comments are a sheet over the bottom of the screen, which a button in the
  header and a tap on a comment's highlighted passage show.
  From 1024 CSS pixels wide, the comments are a column beside the file, which
  stays in view while the file scrolls, and from 1152 an outline of the file's
  sections sits before it.
  The file's text is set as a page of the documentation site is, in a column as
  wide, with the site's type, measures, and colors.
  The comments listed beside it never open gaps in it or stretch its lines.
- **Touch.** Under a finger (`pointer: coarse`), buttons and card actions are at
  least 44 by 44 CSS pixels, Apple's minimum touch target; under a mouse they
  are smaller.
  Nothing depends on hover.
- **Selection.** iOS shows its own callout (Copy, Look Up, Share) above a
  selection, and below one at the top of the screen.
  The actions for a selection show under it, clear of the handle at its end and
  of the callout (see [Commenting](#commenting)).
- **Keyboard.** The composer opens as a bottom sheet sized to the visual
  viewport, so the on-screen keyboard never covers the text being written.
  On a wide screen it opens where the reader asked for it: in the file's text
  under the passage it is about, or under the button that opened it (see
  [Commenting](#commenting)).
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

- **Header.** One row on every screen: the review's title, a picker of the file
  shown when the review tracks several, a picker of what the page shows of it
  (a version, or what a pass or two versions changed), the pass running, and
  **Send** or **Stop**, or **Reopen** on a closed review.
  Every number in it says what it counts: `Version 3`, `Send 2 comments`.
  A narrow screen shortens the labels, and adds a button, a speech bubble with
  the number of comments, that shows and hides the comments.
- **File pane.** Renders the selected file at the active version (defaulting to
  the latest version) using `md-html`.
  Anchored ranges are highlighted in place using distinct styles for `draft` and
  `open` comments; `resolved` comments are hidden, with their highlights, until
  toggled on in the comments panel.
- **Comments panel.** One card per comment on the active file: comments on the
  review, the whole file, and outdated text first, as written, and then comments
  on a range, in the order their ranges start in the file.
  Each card shows the comment's author, status, the start of the text it is on,
  its rendered body, any replacement suggestion (diffing the anchored source
  text against the proposed replacement), and the reply thread with assistant
  verdicts.
  Card actions match the comment's state: edit or withdraw a draft comment, add
  or edit a draft reply on an open comment, resolve an open comment, or reopen a
  resolved one.
- **Picking.** Clicking a card brings out its passage in the file and scrolls
  the file to it, and clicking a highlighted passage brings out its card and
  scrolls the comments to it; on a narrow screen that also shows the comments.
  On a wide screen the comment picked also opens in the file's text, right under
  its passage, as a thread with its replies and actions.
  A highlight is not an element of the page, so a click is matched to the
  passages whose text is drawn under it: the shortest one first, and each
  further click on the same text the next, so every comment on it is reached.
- **Links.** A link in the file is followed as the file means it, not as the
  browser would resolve it against the review's address.
  A `#fragment` scrolls to the heading with that anchor: the server gives each
  Markdown heading the `id` the documentation site gives it, so a link written
  for the site lands on the same heading here, and an address ending in a
  fragment opens at its heading.
  A relative link to another file of the review shows that file, from its start
  or at the heading the link names.
  An address of its own (`https:`, `mailto:`) opens in a new tab, which keeps
  the review open.
  A link to any other workspace file is not followed, and a notice says the file
  is not part of the review: the page shows a review's files and nothing else of
  the workspace.
- **Outdated comments.** A comment whose text changed in a later version shows
  `last seen in version M`, linking to version `M` (the `resolved` projection
  the API already returns) with the range highlighted.
- **Working-tree changes notice.** When any tracked file on disk differs from
  the review's latest recorded version and no pass is running, a notice over the
  file offers to snapshot the working tree as a new version and switch the view
  to it.
  While a pass runs, its post-pass version records the assistant's edits.

### Commenting

Selecting text in the file pane shows its actions beside the selection,
**Comment** and **Suggest** among them:

- A selection dragged with a mouse gets them where the mouse let go, clear of
  the selected text: under the selection's last line after a drag forward, and
  over its first line after a drag backward, at the pointer's place along the
  line.
  The actions are measured as drawn before they are placed, and a line counts as
  tall as the browser draws it selected, which is taller than its letters.
- While the actions show, the browser's own selection marks the range.
  Once the composer opens on it, the selection is cleared and the range is
  highlighted instead, so it is never marked twice.
- A selection made with a touch screen's handles or the keyboard gets them
  under the selection, 30 CSS pixels down to clear the handle at its end.
  They leave room for the iOS callout where it is drawn under the selection, and
  go over the selection, above the callout, when there is no room under it.
  These distances are [Plannotator]'s, which it measured on iOS.
- Deselecting, or pressing Escape, takes the actions away.
- The client maps the DOM selection to half-open UTF-8 source byte offsets using
  the `data-s` and `data-e` spans produced by `md-html` ([RFD 124]), snapping
  `data-atom` spans to their boundaries.
  If either endpoint lies inside an `.unmapped` span, a note in place of the
  actions explains why the selection cannot be anchored.
- The review, file, version, and byte range are captured together when the
  selection is made.
  The suggestion preview and the saved comment use that captured version, not
  whichever version the pane shows when the comment is saved.
  If the pane re-renders before the composer opens, the selection is dropped and
  the actions hide.
- **Comment** opens the composer for the Markdown comment body.
  **Suggest** opens it with an editable replacement box, pre-filled with the raw
  source slice covered by the byte range (including any Markdown syntax markers
  such as `**` that do not appear in the rendered text).
- On a wide screen the composer for what is about a passage (a new comment on a
  range, an edit of one, a reply to one) opens in the flow of the file's text,
  as wide as the text, right under what the range ends in: after the paragraph,
  heading, table, or code block, or inside the list item, under the item's own
  text and before the lists nested in it.
  A list is one block however long it is, so the end of the list would be far
  from most of its items.
  The text below moves down to make room, and the composer takes the keyboard.
  The composer's element is moved to that place rather than rendered there, so
  the file's own elements stay as the server rendered them.
- Saving submits `POST /api/reviews/{id}/comments` with the captured `version`
  and `bytes`.
  Command or Control with Enter saves from either field.

A comment on no passage starts from **Add comment** in the comments panel, the
one place for it.
In a review of one file it is a comment on the review; in a review of several,
the panel first asks whether the file shown or the review is meant.
On a wide screen its composer opens right under that button, and the one for a
pass message under **Send**.
While a selection is pending or the composer is open, the file pane does not
switch versions on its own.

### Approving and Reading

Two more actions on a selection record what the reviewer made of text they have
no comment on, as [RFD 136] defines them, and every pass tells the assistant
both:

- **Approve** marks the passage approved and highlights it.
  Clicking an approved passage offers to take the approval back.
  Once the passage changes in a later version it loses its highlight, and the
  comments panel lists it as changed since it was approved.
- **Read to here** draws a line across the file under the block the selection
  ends in: the reviewer has read this far, and no further.
  A file has one such line, which setting it again moves.

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
- When the pass finishes, the comments panel shows the pass outcome and the
  assistant's closing message over the cards, with the earlier passes folded
  under them, and the file pane switches to the changes view for that pass
  (unless the reviewer is actively composing a comment or reply).

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
- The header's picker lists each completed pass, and the comments panel links
  each to its changes view.
- Clicking a card in the changes view returns to the file, at the card's
  passage.
- A file deleted in the `to` version shows as deleted, with its unified diff.
- Changed lines are highlighted through the rendered text on them, so a line
  whose only change is invisible, such as a link target, still highlights its
  visible text.
  Changed lines with no rendered text at all, such as a reference definition or
  an HTML comment, are counted over the file ("2 changed lines not shown") with
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
| Each heading of a file rendered as Markdown carries, as its `id`, the anchor the documentation site gives it (`doc::heading`), and the file's response lists its headings.                                                                                                                    | Following `#fragment` links in a file, and its outline.      |
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
- **A bar fixed to the bottom of the screen for a selection's actions.** Works
  the same on every screen and never meets the iOS callout, but on a wide
  screen it is far from the selection the mouse just made.
- **The composer always beside the file.** One place for every kind of draft,
  but a new comment is then written in a narrow column away from the text it is
  about.
- **The composer floating over the text under the selection.** Sits exactly
  under the selected line and moves nothing, but covers the lines that follow,
  which are often what the comment is about.
- **Rewriting a file's links on the server.** The server knows the review's
  files as well as the client does, but what a link leads to depends on the page
  showing it, and the client already resolves a selection against the same
  file path.
- **Every comment card inline under the block it is on, and in a margin beside
  it on wide screens.** Keeps a comment next to its text without picking.
  A card sharing a row with its block stretches the row, which opens gaps in
  the text, and on a phone a long thread pushes the text it is about off the
  screen.
  Only the comment picked opens under its passage, and only on a wide screen.
  Placing cards beside their passages without stretching the text, as Google
  Docs does, needs every passage and card measured and their collisions
  resolved on each resize; it can be added to the panel later.
- **Header rows for file tabs, pass history, and view tools.** Shows more at
  once, at the cost of a header that took a third of a phone's screen and
  several buttons whose scope depended on the row they were in.

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
- **Picking a passage on iOS.** A tap is matched to a passage by where the
  passage's text is drawn, which needs iOS to deliver the tap on the file's text
  as a click; this is checked on an iPhone.
- **Placement beside the iOS callout.** The distances that keep a selection's
  actions clear of the callout and the handle are Plannotator's, and have not
  been measured again in this UI.
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
mobile-first layout (a single column with the comments in a sheet, and beside
the file from 1024 CSS pixels), CSS Custom Highlight ranges, the version
switcher, outdated-comment links, the rendered and unified diff views, and
polling.
Add the server's address and lock file, `serve --open`, `start --open`, and
`open`, which need a UI to open.
The rest of [RFD 139]'s plan (the wasm target, `just build-ui`, the `wasm_check`
and `wasm_test` tool options, and the `web` CI workflow) lands here, with the
first `ui` crate.
Depends on Phase 1 and [RFD 139].

### Phase 3: Interactive Commenting and Pass Controls

Add selection-to-bytes mapping and the actions beside a selection, the comment
and suggestion composer, in the file's text on a wide screen and as a bottom
sheet on a narrow one, which survives a reload, **Add comment** for file-level
and review-level comments, picking between cards and passages, links and heading
anchors in a file, draft editing and withdrawal, replies and comment resolution,
working-tree snapshot recording from the notice, and pass submission and
stopping.
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
