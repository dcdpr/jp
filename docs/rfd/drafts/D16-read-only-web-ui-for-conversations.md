# RFD D16: Read-Only Web UI for Conversations

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-06
- **Summary**: Serves a JavaScript-free, mobile-first read-only HTML interface
  for browsing conversations via `jp serve --web`.

## Summary

This RFD introduces a read-only web interface for browsing JP conversations on a
mobile device.
`jp serve --web` starts an HTTP server that renders HTML on the server using
Axum and Maud, styled with mobile-first CSS, automatic dark mode, and no
JavaScript.

## Motivation

JP conversations can only be read in the terminal via `jp conversation print`.
This works at a workstation, but not when away from the machine, such as on a
phone.

A lightweight web UI that renders conversations as HTML solves this.
Because the primary use case is reading on a phone at night, the UI needs a
mobile-first layout, dark mode, and no custom JavaScript controls so native iOS
and Android text interactions (zoom, select, copy-paste) work unimpeded.

## Design

### User Experience

The UI uses a two-pane chat layout:

- **Conversation list** (left pane / drawer): Lists all conversations sorted by
  last activity, with each entry's title and timestamp.
  This is the default view on mobile; tapping a conversation opens the detail
  view.
- **Conversation detail** (right pane / main view): Displays the full
  conversation history as a scrollable chat view with visually distinct user
  messages and assistant responses, collapsible tool-call blocks, and Markdown
  rendered to HTML with syntax-highlighted code blocks.
- **Navigation**: On mobile, a CSS-only toggle slides the sidebar in and out
  without JavaScript.
  On wider viewports, the sidebar stays visible.

### CLI Entry Point

A new top-level `serve` subcommand starts the server:

```
jp serve --web                    # default: 127.0.0.1:3141
jp serve --web --port 8080
jp serve --web --bind 0.0.0.0    # for phone access over LAN
```

The command initializes the workspace through the standard startup pipeline and
runs an HTTP server until interrupted.
`serve` sits at the top level rather than under `conversation` so it can also
host the future `--http-api` endpoint.

### Configuration

`AppConfig` gains a `ServerConfig` struct with a nested `WebServerConfig`,
exposed in TOML under `server.web`:

```toml
[server.web]
bind = "127.0.0.1" # listening address
port = 3141 # listening port
```

The `server` namespace leaves room for future sections such as
`server.http_api`.

### Routes

| Route                    | Description                                      |
| ------------------------ | ------------------------------------------------ |
| `GET /`                  | Redirect to `/conversations`                     |
| `GET /conversations`     | List conversations sorted by `last_activated_at` |
| `GET /conversations/:id` | Render a single conversation's chat history      |
| `GET /assets/style.css`  | Serve embedded CSS with cache headers            |

All page routes return server-rendered HTML.
The `:id` parameter accepts the conversation's decisecond timestamp identifier
(the same format used by `jp conversation show`).

### Architecture

#### New Crate: `jp_web`

```
crates/jp_web/
├── Cargo.toml
└── src/
    ├── lib.rs          # start_server(workspace, config) -> Result
    ├── state.rs        # Shared application state (Arc<Workspace>, etc.)
    ├── routes.rs       # Axum router and handler functions
    ├── views/
    │   ├── layout.rs   # Base HTML shell (maud)
    │   ├── list.rs     # Conversation list page
    │   └── detail.rs   # Conversation detail page
    ├── render.rs       # Event rendering: ConversationStream → HTML
    └── style.rs        # Embedded CSS via include_str!()
```

The crate depends on `jp_workspace`, `jp_conversation`, and `jp_config` for data
access, and on `axum`, `maud`, `comrak`, and `syntect` for serving and
rendering.

#### Technology Choices

**Axum** (0.8): HTTP framework built on tokio and tower.
Because JP already enables tokio's full feature set, axum reuses the existing
async runtime with minimal additional dependency surface.

**Maud** (0.27): Compile-time macro that generates HTML from Rust expressions
with automatic escaping.
Its transitive dependencies (`itoa`, `proc-macro2`, `quote`, and `syn`) are
already in the workspace dependency tree, leaving `proc-macro2-diagnostics` as
the only net-new crate.
For a 2-3 page UI, co-locating HTML structure with Rust rendering logic,
checking types at compile time, and avoiding runtime template registration
outweigh that single dependency compared to minijinja.

**Comrak** (existing): Workspace dependency that renders Markdown message
content to HTML via `markdown_to_html()`.

**Syntect** (existing): Workspace dependency that highlights code blocks with
HTML `<span>` elements instead of the ANSI escape sequences used by the terminal
renderer.

#### Rendering Pipeline

The web renderer follows the same structure as
`jp_cli::cmd::conversation::print`, targeting HTML instead of ANSI:

1. Load conversation events via `Workspace::events(handle)`.
2. Iterate turns via `ConversationStream::iter_turns()`.
3. Render each event:
   - `ChatRequest`: user message bubble, with Markdown converted to HTML via
     comrak.
   - `ChatResponse::Message`: assistant message bubble.
   - `ChatResponse::Reasoning`: collapsible `<details>` block, respecting the
     `style.reasoning.display` config.
   - `ChatResponse::Structured`: `<pre>` JSON block.
   - `ToolCallRequest` / `ToolCallResponse`: collapsible `<details>` block
     showing the tool name, arguments, and result.
     Tools marked hidden by `conversation.tools.<name>.style.hidden` are
     skipped.
   - `InquiryRequest` / `InquiryResponse`: skipped, matching terminal print.

#### CSS Strategy

A single CSS file is embedded via `include_str!()` and served at
`/assets/style.css` with a content-hash ETag for cache busting:

- **Dark mode**: `@media (prefers-color-scheme: dark)` follows the OS setting
  automatically without a JavaScript toggle.
- **Mobile-first**: The default layout is a single column, with the sidebar
  toggled as a drawer via a CSS checkbox hack (`<input type="checkbox">` +
  `<label>` + sibling selectors) and no JavaScript.
- **Responsive breakpoint**: At wider viewports (e.g. `>768px`), the sidebar
  remains visible alongside the conversation.
- **Native interactions**: A standard viewport meta tag (`width=device-width,
  initial-scale=1`) and the absence of JavaScript preserve native iOS text
  selection, zoom, and copy-paste.
- **Code blocks**: Syntect emits inline `style` attributes for syntax
  highlighting, with dark and light themes selected via CSS custom properties or
  the `prefers-color-scheme` media query.

### Workspace Access

Axum handlers share the `Workspace` struct via `Arc`.
The web server opens the workspace in read-only mode, never acquiring
conversation locks or writing to disk.
Conversation metadata and events load lazily on request through `Workspace`'s
existing `OnceLock`-based lazy loading.

When a `jp query` session runs concurrently in the same workspace, the web
server reads a consistent snapshot of the state last persisted to disk.
Because the server only reads, the two processes do not conflict.

## Drawbacks

- **New binary surface area**: The axum dependency adds HTTP server code to the
  JP binary, increasing binary size and widening the attack surface with a
  network HTTP listener.
- **No live updates**: The MVP serves static HTML on each page load.
  If `jp query` is actively writing to a conversation, the web view shows stale
  data until the page is refreshed.
- **Single workspace**: `jp serve` serves conversations only from the workspace
  where it is run; accessing multiple workspaces requires running multiple
  servers.

## Alternatives

### Minijinja instead of Maud

Minijinja is already a workspace dependency and would add no new crates.
However, maud adds only one net-new crate (`proc-macro2-diagnostics`) while
providing compile-time HTML type checking, automatic escaping, and markup
co-located with rendering logic.
For a 2-3 page UI, those benefits justify the single extra dependency.

### Askama instead of Maud

Askama compiles file-based Jinja2 templates at build time, which suits projects
with many templates or non-Rust contributors editing HTML.
For 2-3 pages authored by Rust developers, maud's inline macros are more
ergonomic and avoid maintaining separate template files.

### Static site export instead of a server

A `jp conversation export --html` command could write static HTML files without
running a server, but would require re-exporting after each conversation update.
A server serves current data with no manual step.
Static export can be added later as a complementary feature.

## Non-Goals

- **Write support**: Sending messages or editing conversations through the web
  UI is future work; this RFD covers read-only access.
- **Authentication**: The server binds to localhost by default and is accessible
  to anyone on the LAN when bound to `0.0.0.0`.
  Authentication is deferred to a future RFD.
- **HTTP API**: A JSON API (`jp serve --http-api`) is planned but out of scope;
  the `serve` subcommand and `server` config namespace are structured to
  accommodate it.
- **Real-time streaming**: Live updates via SSE or WebSocket are future work;
  the MVP serves a static snapshot on each page load.
- **Theming / custom CSS**: The built-in CSS supports light and dark mode; user
  customization of colors or layout is out of scope.

## Risks and Open Questions

- **Concurrent workspace access**: The web server reads from the same on-disk
  storage that `jp query` writes to.
  Storage uses append-only JSON files with file-level write locks, so lock-free
  reads need validation, particularly around partially written event files.
- **Binary size impact**: Axum and its tower/hyper transitive dependencies
  increase binary size.
  Measure the increase after Phase 1 to decide whether to gate the web server
  behind a Cargo feature flag.
- **Code highlighting theme parity**: The terminal uses syntect with ANSI output
  while the web renderer uses syntect with HTML output, so themes may not map
  1:1 and the dark/light theme pairing needs testing.

## Implementation Plan

### Phase 1: Crate skeleton and server infrastructure

- Create the `jp_web` crate with axum server setup.
- Add the `server.web` config section (`bind`, `port`) to `jp_config`.
- Add the `jp serve --web` subcommand to `jp_cli`.
- Serve a placeholder HTML page at `GET /` to validate the pipeline end-to-end.
- Review and merge independently.

### Phase 2: Conversation list

- Implement the `GET /conversations` route.
- Load all conversation metadata via `Workspace::conversations()`.
- Render the list with each conversation's title, timestamp, and link to the
  detail view.
- Build the Maud layout with embedded mobile-first, dark-mode CSS.

### Phase 3: Conversation detail

- Implement the `GET /conversations/:id` route.
- Render conversation events as a chat view, converting Markdown to HTML via
  comrak and highlighting code via syntect.
- Render tool calls as collapsible `<details>` blocks.
- Add sidebar navigation back to the list.

### Phase 4: Polish

- Add the responsive sidebar (CSS-only drawer on mobile, persistent on desktop).
- Serve the CSS asset with cache headers.
- Test on iOS Safari to verify zoom, select, and copy behavior.
- Measure the binary size impact and decide on feature-flag gating.

## References

- [Axum documentation][axum]
- [Maud documentation][maud]
- [Comrak HTML rendering][comrak]

[axum]: https://docs.rs/axum/0.8
[comrak]: https://docs.rs/comrak
[maud]: https://maud.lambda.xyz/
