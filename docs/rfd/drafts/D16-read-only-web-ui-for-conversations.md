# RFD D16: Read-Only Web UI for Conversations

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-06

## Summary

This RFD introduces a read-only web interface for browsing JP conversations from
a mobile device. The server is started via `jp serve --web` and serves
server-rendered HTML using Axum and Maud, with a mobile-first CSS layout that
supports automatic dark mode. No JavaScript is required.

## Motivation

JP conversations are currently only readable through the terminal via `jp
conversation print`. This works well at a workstation but is inaccessible when
away from the machine — most commonly when reading from a phone.

A lightweight web UI that renders conversations as HTML solves this. It needs to
be mobile-first (the primary use case is reading on a phone at night), support
dark mode, and preserve the native iOS/Android text interactions (zoom, select,
copy-paste) by avoiding custom JavaScript controls.

## Design

### User Experience

The UI follows the standard chat-app layout:

- **Conversation list** (left pane / drawer): Shows all conversations sorted by
  last activity, displaying title and timestamp. On mobile this is the default
  view; tapping a conversation navigates to the detail view.
- **Conversation detail** (right pane / main view): Renders the full
  conversation history as a scrollable chat view. User messages and assistant
  responses are visually distinct. Tool calls are rendered as collapsible
  blocks. Markdown content is rendered to HTML with syntax-highlighted code
  blocks.
- **Navigation**: The sidebar slides in/out on mobile via a CSS-only toggle (no
  JavaScript). On wider viewports, the sidebar is always visible.

### CLI Entry Point

A new `serve` subcommand is added to the CLI:

```
jp serve --web                    # default: 127.0.0.1:3141
jp serve --web --port 8080
jp serve --web --bind 0.0.0.0    # for phone access over LAN
```

The command initializes the workspace using the standard startup pipeline, then
starts an HTTP server that blocks until interrupted. The `serve` subcommand is a
top-level command (not nested under `conversation`) because it will later host
the `--http-api` endpoint as well.

### Configuration

A new top-level `server.web` config section:

```toml
[server.web]
bind = "127.0.0.1" # listening address
port = 3141 # listening port
```

This is added to `AppConfig` via a new `ServerConfig` struct containing a
nested `WebServerConfig`. The `server` namespace is reserved for future
additions like `server.http_api`.

### Routes

| Route                      | Description                                          |
|----------------------------|------------------------------------------------------|
| `GET /`                    | Redirect to `/conversations`                         |
| `GET /conversations`       | Conversation list, sorted by `last_activated_at`     |
| `GET /conversations/:id`   | Render a single conversation's chat history          |
| `GET /assets/style.css`    | Embedded CSS, served with cache headers              |

All routes return server-rendered HTML. The `:id` parameter accepts the
conversation's decisecond timestamp identifier (the same format used by
`jp conversation show`).

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

The crate depends on `jp_workspace`, `jp_conversation`, and `jp_config` for
data access, and on `axum`, `maud`, `comrak`, and `syntect` for serving and
rendering.

#### Technology Choices

**Axum** (0.8): HTTP framework built on tokio and tower. JP already depends on
tokio with full features enabled, so axum reuses the existing async runtime
with minimal additional dependency surface.

**Maud** (0.27): Compile-time HTML macro. Generates HTML from Rust expressions
with automatic escaping. The transitive dependency cost is near-zero — `maud`
pulls in `itoa`, `proc-macro2`, `quote`, and `syn`, all of which are already
in the dependency tree. The only net-new crate is `proc-macro2-diagnostics`.
Maud is preferred over minijinja for this use case because it keeps the HTML
structure co-located with the Rust rendering logic, provides compile-time type
checking, and avoids runtime template registration — advantages that outweigh
the minor additional dependency for a UI with only 2-3 pages.

**Comrak** (existing): Already a workspace dependency. Used in HTML output mode
(`markdown_to_html()`) to render conversation message content.

**Syntect** (existing): Already a workspace dependency. Used to produce
`<span>`-based syntax highlighting for code blocks in HTML output (instead of
the ANSI escape sequences used by the terminal renderer).

#### Rendering Pipeline

The web renderer mirrors the logic in `jp_cli::cmd::conversation::print` but
targets HTML instead of ANSI:

1. Load conversation events via `Workspace::events(handle)`.
2. Iterate turns via `ConversationStream::iter_turns()`.
3. For each event:
   - `ChatRequest` → render user message bubble (markdown → HTML via comrak).
   - `ChatResponse::Message` → render assistant message bubble.
   - `ChatResponse::Reasoning` → render as a collapsible `<details>` block
     (respecting `style.reasoning.display` config).
   - `ChatResponse::Structured` → render as a `<pre>` JSON block.
   - `ToolCallRequest` / `ToolCallResponse` → render as a collapsible
     `<details>` block showing tool name, arguments, and result. Hidden tools
     (per `conversation.tools.<name>.style.hidden`) are skipped.
   - `InquiryRequest` / `InquiryResponse` → skipped (same as terminal print).

#### CSS Strategy

A single CSS file embedded via `include_str!()` and served at
`/assets/style.css` with a content-hash ETag for cache busting.

Key properties:

- **Dark mode**: `@media (prefers-color-scheme: dark)` — follows the OS
  setting automatically, no JavaScript toggle.
- **Mobile-first**: Default layout is single-column. Sidebar appears as a
  drawer toggled via a CSS checkbox hack (`<input type="checkbox">` +
  `<label>` + sibling selectors). No JavaScript.
- **Responsive breakpoint**: At wider viewports (e.g. `>768px`), the sidebar
  is always visible alongside the conversation.
- **Native interactions**: Standard viewport meta tag
  (`width=device-width, initial-scale=1`). No JavaScript that could interfere
  with iOS text selection, zoom, or copy-paste.
- **Code blocks**: Syntax highlighting via inline `style` attributes from
  syntect. Dark/light themes selected via CSS custom properties or the
  `prefers-color-scheme` media query.

### Workspace Access

The `Workspace` struct is shared with the axum handlers via `Arc`. The web
server opens the workspace in read-only mode — it never acquires conversation
locks or writes to disk. Conversation metadata and events are loaded lazily
on request (the existing `OnceLock`-based lazy loading in `Workspace` handles
this).

If the workspace is also being used by a concurrent `jp query` session, the
web server sees a consistent snapshot of whatever was last persisted to disk.
There is no conflict because the web server only reads.

## Drawbacks

- **New binary surface area**: The axum dependency adds HTTP server code to the
  JP binary. This increases binary size and widens the attack surface (an HTTP
  listener on the network).
- **No live updates**: The MVP serves static HTML on each page load. If a
  conversation is actively being written to by `jp query`, the web view shows
  stale data until the page is refreshed.
- **Single workspace**: The server serves conversations from a single workspace
  (the one where `jp serve` is run). Accessing conversations from multiple
  workspaces requires running multiple servers.

## Alternatives

### Minijinja instead of Maud

Minijinja is already a workspace dependency, so using it would add zero new
crates. However, maud's transitive dependency cost is near-zero (one net-new
crate: `proc-macro2-diagnostics`), and it provides compile-time HTML type
checking, automatic escaping, and co-location of markup with rendering logic.
For a small UI with 2-3 pages, the ergonomic benefits justify the minimal
dependency cost.

### Askama instead of Maud

Askama uses file-based Jinja2 templates compiled at build time. It is better
suited for projects with many templates or where non-Rust contributors edit
HTML. For 2-3 pages authored by Rust developers, maud's inline approach is
more ergonomic and avoids the overhead of maintaining separate template files.

### Static site export instead of a server

An alternative is `jp conversation export --html` that writes static HTML
files. This avoids running a server but requires re-exporting after each
conversation update. A server provides always-current data with no manual step.
A static export could be added later as a complementary feature.

## Non-Goals

- **Write support**: This RFD covers read-only access. Sending messages or
  editing conversations through the web UI is future work.
- **Authentication**: The server binds to localhost by default. When bound to
  `0.0.0.0`, it is accessible to anyone on the LAN. Adding authentication is
  deferred to a future RFD.
- **HTTP API**: A JSON API (`jp serve --http-api`) is planned but out of scope
  for this RFD. The `serve` subcommand and `server` config namespace are
  designed to accommodate it.
- **Real-time streaming**: Live-updating conversations via SSE or WebSocket is
  future work. The MVP serves a static snapshot on each page load.
- **Theming / custom CSS**: The built-in CSS supports light and dark mode. User
  customization of colors or layout is not in scope.

## Risks and Open Questions

- **Concurrent workspace access**: The web server reads from the same on-disk
  storage that `jp query` writes to. The current storage format uses
  append-only JSON files with file-level locking for writes. Read-only access
  without locks should be safe, but this needs validation — particularly
  around partially-written event files.
- **Binary size impact**: Axum and its tower/hyper transitive dependencies will
  increase binary size. This should be measured after Phase 1 to decide
  whether the web server should be gated behind a cargo feature flag.
- **Code highlighting theme parity**: The terminal uses syntect with ANSI
  output. The web version uses syntect with HTML output. The themes may not
  map 1:1. The dark/light theme pairing needs testing.

## Implementation Plan

### Phase 1: Crate skeleton and server infrastructure

- Create `jp_web` crate with axum server setup.
- Add `server.web` config section to `jp_config` (`bind`, `port`).
- Add `jp serve --web` subcommand to `jp_cli`.
- Serve a placeholder HTML page at `GET /` to validate the pipeline end-to-end.
- Can be reviewed and merged independently.

### Phase 2: Conversation list

- Implement `GET /conversations` route.
- Load all conversation metadata via `Workspace::conversations()`.
- Render as a list with title, timestamp, and link to detail view.
- Maud layout with embedded CSS (mobile-first, dark mode).

### Phase 3: Conversation detail

- Implement `GET /conversations/:id` route.
- Render conversation events as a chat-style view.
- Markdown → HTML via comrak. Code highlighting via syntect.
- Tool calls as collapsible `<details>` blocks.
- Sidebar navigation back to list.

### Phase 4: Polish

- Responsive sidebar (CSS-only drawer on mobile, persistent on desktop).
- Cache headers for CSS asset.
- Test on iOS Safari to verify zoom/select/copy behavior.
- Measure binary size impact and decide on feature-flag gating.

## References

- [Axum documentation][axum]
- [Maud documentation][maud]
- [Comrak HTML rendering][comrak]

[axum]: https://docs.rs/axum/0.8
[maud]: https://maud.lambda.xyz/
[comrak]: https://docs.rs/comrak
