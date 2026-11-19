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

`jp review serve` serves a browser UI for reviewing files with the assistant.
The reviewer reads the files rendered, selects text to comment on it or suggest
a replacement, sends passes and watches the assistant's replies arrive, and sees
what each pass changed, marked in the rendered file.

## Motivation

[RFD 136] built the review engine, its CLI, and an HTTP API for a browser
frontend it left out of scope.
The API is shaped for one: anchors given as byte ranges, `revision` for polling,
and files rendered by md-html with their source offsets ([RFD 124]).

From the terminal, a comment on a phrase needs its line and column range typed
out, `--at docs/notes.md:42:5-43:18`, counted in a file the reviewer is reading
rendered somewhere else.
Selecting the phrase on the rendered page is the step a browser makes cheap.

The other question a review keeps asking is what the assistant changed in its
last pass.
`jp review diff --pass last` answers with a unified diff of the Markdown source;
for prose, the same change marked in the rendered file is faster to read.

## Design

The UI is a Leptos client in `jp-review-ui`, built and embedded in `jp-review`
as [RFD 139] describes, with its wire types in `jp-review-api`.

### Opening the UI

```sh
jp review serve --open                # start the server, open the review list
jp review start docs/notes.md --open  # start a review and open it
jp review open R-1h4k2m9              # open a review in the running server
```

- `jp review serve` writes its address into the review store and holds a lock on
  that file while it runs.
  A store has one server: a second `serve` refuses to start and names the
  running one.
- `jp review open [ID]` opens the review list, or one review, in the default
  browser.
  When the lock is free, no server runs, and `open` says how to start one.
- `start --open` starts the review, then opens it.

### The review list

`/` lists the reviews with their titles, files, latest versions, comment counts,
and whether a pass is running.
A form starts a review of paths relative to the workspace root.

### The review page

`/reviews/R-1h4k2m9` shows one review:

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

- **File pane.** The selected file at the displayed version, the latest by
  default, rendered by md-html.
  Commented ranges are highlighted in place: drafts, open comments, and resolved
  ones, muted and hidden until shown.
- **Margin.** One card per comment on the file, beside its highlight; comments
  on the whole file or review sit above the first line.
  On a narrow screen, each card sits below the block its anchor starts in.
- **Cards.** A card shows the comment's author, status, body, and suggestion
  (the source text it covers against its replacement), then the replies with
  their verdicts.
  Its actions follow its state: edit or withdraw a draft, write or change the
  draft reply on an open comment, resolve.
- **Outdated comments** say "outdated since version N"; the link opens version N
  with the comment's range highlighted.
- **Files changed on disk.** When the tracked files differ from the latest
  version, a banner offers to record them as a new version, which the page then
  shows.

### Commenting

Selecting text in the file pane shows a Comment button by the selection.

- The selection maps to source bytes as [RFD 124] describes, through the
  `data-s` and `data-e` spans, with atoms snapping to their edges.
  A selection ending in unmapped text can't be commented on, and the button says
  why.
- The composer takes a Markdown body.
  "Suggest a change" adds a replacement box holding the source text the
  selection covers, which can include Markdown the page hides, such as `**`.
- Saving creates a draft comment anchored by `bytes` in the displayed version.

Buttons on the file tab and in the header start a comment on the file or on the
review.

### Passes

- **Send** sends everything waiting on the assistant, with an optional message,
  as `jp review submit` does without `--only`.
- While the pass runs, the header shows it with a Stop button, and each reply
  and comment the assistant stores appears as it arrives.
- When it ends, the header shows the outcome and the assistant's closing
  message, and the page shows what the pass changed, unless the reviewer is
  writing a comment.

The closing message is the last assistant message of the pass's turn.
The process running the pass reads it from the conversation through the host
when the turn ends, and stores it with the pass's outcome in `review.json`, so
`jp review show` prints it as well.

### Changes

The changes view shows a file at one version with what changed since an earlier
one marked: added and rewritten lines highlighted in the rendered file, and a
marker where lines were removed.
A toggle shows the unified source diff instead.

- **What pass N changed** compares the version the pass was sent against with
  the one recorded when it ended, as `jp review diff --pass N` does.
- Two version pickers compare any two versions.
- The header's pass list links each pass to its changes.

### Updates

The page polls `GET /api/reviews/{id}?revision=N` every 2 seconds, every 500
milliseconds while a pass runs, and not at all while its tab is hidden.

### Server changes

Additions to [RFD 136]'s API:

| Change                                                                                                                                                       | For                                                         |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------- |
| Each file in `GET …/diff` lists the byte ranges, in the `to` version, of the lines the change added or rewrote, and the byte offsets where it removed lines. | Marking changes in the rendered file.                       |
| `GET /api/reviews/{id}` says whether the files on disk differ from the latest version, and `POST /api/reviews/{id}/versions` records them.                   | The banner.                                                 |
| Comment and reply records carry `body_html`, the body rendered by md-html.                                                                                   | Rendering bodies without a Markdown renderer in the bundle. |
| A pass's outcome carries the assistant's closing message.                                                                                                    | The pass panel, and `jp review show`.                       |

A file in a diff:

```json
{
  "path": "docs/notes.md",
  "change": "modified",
  "diff": "@@ -40,3 +40,3 @@\n...",
  "added": [[4113, 4148]],
  "removed": [3980]
}
```

The request and response types move into `jp-review-api`; the JSON existing
endpoints return doesn't change.

Routes beside the API:

- `/` and `/reviews/…` serve the HTML shell, and the bundle routes between
  them.
- `/ui/…` serves the bundle: the WebAssembly module, the module `wasm-bindgen`
  generated for it, and the stylesheet.
- A server built without the bundle answers the shell's routes with the
  placeholder page.

### Security

- The UI calls only the API, through the same server and its existing checks:
  loopback by default, `allowed_hosts`, and the same-origin check on writes.
  It gives a page nothing the API doesn't.
- The shell is served with a Content-Security-Policy allowing scripts from the
  server's own origin, the one inline loader by its hash, and WebAssembly
  compilation (`'wasm-unsafe-eval'`), and nothing from other origins.
- File contents and comment bodies reach the page as md-html output, which
  escapes raw HTML and drops `javascript:` and similar link schemes.

## Drawbacks

- Polling: a reply shows up to half a second after it is stored, and an idle
  open tab asks every 2 seconds.
- The page shows recorded versions, not the files as they are on disk; edits
  made in an editor are recorded from the banner before they can be commented
  on.
- A server built without `just install-plugin review` shows the placeholder.
- jp-review grows two crates.

## Alternatives

- **Server-sent events instead of polling.** The assistant's replies are written
  to `review.json` by tool processes, not by the server, so pushing them means
  watching the store; polling `revision` is what the API already supports.
  Worth revisiting if polling shows up as load.
- **Recording a version whenever a file changes on disk.** Every save in an
  editor would add a version, and outdate comments anchored to the one before.
- **Rendering Markdown in the browser**, with comrak compiled to WebAssembly: a
  larger bundle for what the server already renders.
- **Changes as source diffs only**: what `jp review diff` already gives, and
  hard to read for prose.

## Non-Goals

- Editing files, or applying suggestions, in the browser ([RFD 136]).
- Choosing which comments a pass sends; the page sends everything pending.
- Reviewing commits or assistant messages ([RFD 136]).
- Opening a review from the documentation site.
- Authentication and multiple reviewers.
- Moving `just review-file` onto the UI.

## Risks and Open Questions

- **Browsers.** The Custom Highlight API shipped in Chrome 105, Safari 17.2, and
  Firefox 140; an older browser shows the cards without highlights.
- **Crowded margins.** Cards anchored to nearby lines stack, and can drift away
  from their highlights.
- **Rendering.** md-html renders with comrak and the documentation site with
  markdown-it, so a file can look different here than on the site.
- **Touch.** Selection on phones and tablets goes through the browser's own
  selection, and is untested.

## Implementation Plan

### Phase 1: Server

The `jp-review-api` crate, with the wire types moved into it; the API additions,
including the closing message on a pass's outcome; the shell and bundle routes,
the placeholder, and the Content-Security-Policy; and the server's address
record, `serve --open`, `start --open`, and `open`.
Mergeable on its own: the shell serves the placeholder until Phase 2.

### Phase 2: Reading

The `jp-review-ui` crate: the review list; the review page with its highlights,
cards, versions, and outdated links; the changes view; and polling.
Requires Phase 1, and the single phase of [RFD 139].

### Phase 3: Writing

Commenting from a selection, suggestions, comments on files and reviews,
replies, resolving, editing and withdrawing drafts, recording versions, and
sending and stopping passes.

## References

- [RFD 124: Source-Mapped Markdown Rendering][RFD 124]
- [RFD 136: Review Engine for Workspace Files][RFD 136]
- [RFD 139: Browser UIs in Rust with Leptos][RFD 139]
- [Plannotator]

[Plannotator]: https://plannotator.ai/
[RFD 124]: 124-source-mapped-markdown-rendering.md
[RFD 136]: 136-review-engine-for-workspace-files.md
[RFD 139]: 139-browser-uis-in-rust-with-leptos.md
