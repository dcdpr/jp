# RFD 099: Native macOS App for Browsing Conversations

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Summary**: Introduces a native SwiftUI macOS conversation browser backed by
  `jp_ffi`, `Workspace::open`, and a `jp-gui` command plugin.

## Summary

This RFD introduces a native SwiftUI macOS app for reading JP conversations,
backed by a thin Rust FFI crate (`jp_ffi`) that links `jp_workspace` and
`jp_conversation` directly into the app process.
It also extracts workspace initialization from private `jp_cli` code into
`Workspace::open`, and adds a `jp-gui` command plugin so `jp gui` opens the
current workspace in the app.

## Motivation

JP can render conversations in the terminal (`jp conversation print`) and in a
browser via the [RFD 072] plugin system (`jp serve web`).
Neither fits long-form browsing: scrolling back through weeks of turns,
comparing conversations side by side, or reading locally on the machine where
the work happened.
A dedicated macOS app fits that workflow, provided it uses native windows, tabs,
menus, keyboard navigation, and state restoration rather than wrapping a web
view.

Building a second consumer also exposes a flaw in the `Workspace` API.
Until now, `jp_cli` has been the sole consumer of `Workspace`, and the
seven-step sequence that initializes a workspace from a filesystem path lives in
a private `load_workspace` function in `jp_cli`:

1. Walk up from a directory looking for `.jp` (`Workspace::find_root`).
2. Load the workspace `Id` from `.jp/`.
3. Build `FsStorageBackend::new(.jp)`.
4. Resolve `user_data_dir()/workspace` as the user-storage root.
5. Wire user storage with `with_user_storage(user_root, slug, id)`.
6. Build the workspace with `Workspace::new_with_id(root, id).with_backend(fs)`.
7. Persist the ID back with `id().store(&storage)`.

Because `load_workspace` is private to `jp_cli`, another consumer must either
depend on `jp_cli` (which pulls in `clap`, the printer, every LLM provider, and
MCP), copy those forty lines, or extract them into `jp_workspace`.

Copying the sequence is error-prone because step 5 is easy to omit.
Without `with_user_storage`, the code compiles and `Workspace::conversations()`
returns results without error, but it silently omits conversations stored in
user-local storage (see [RFD 073]).
This bug already occurred once during [RFD 072], when the web server was first
written without user-local storage wired up.

Furthermore, `Workspace::new(path)` is the most prominent public constructor,
but it returns an in-memory workspace with no filesystem backend.
That constructor exists so tests can build a workspace without touching disk,
and to leave room for future designs that drop the user-storage requirement.
However, naming the in-memory constructor `new` misleads callers looking to open
a workspace from disk.

## Design

### User Experience

The app is a read-only viewer: it opens workspaces and displays conversations,
with no compose field or write operations.

- **Opening.** `File > Open` presents a directory chooser.
  Selecting any directory inside a workspace opens that workspace by walking up
  to find `.jp`, matching `jp`'s behavior.
  `File > Open Recent` lists previously opened workspaces.
- **Windows.** Each workspace opens in its own window and uses native window
  tabbing, so tabs can be pulled out into separate windows.
- **Layout.** Two panes split by a draggable divider: the conversation list on
  the left and the conversation history on the right.
  The divider position is restored across launches.
- **Conversations.** The list sorts by last activity and shows each
  conversation's title and timestamp.
  Double-clicking a conversation opens it in a dedicated window.
- **Terminal integration.** `jp gui` opens the current workspace in the app.

Menus, keyboard shortcuts, selection behavior, copy representations, and drag
sources are designed as a separate deliverable in the Implementation Plan; this
RFD defines the app's architecture and core layout rather than its full
affordance map.

### Repository Layout

```txt
crates/jp_ffi/                 # staticlib: C ABI over jp_workspace + jp_conversation
crates/plugins/command/gui/    # jp-gui: launches the app at a workspace root
apps/macos/                    # Xcode project and Swift sources (not a cargo member)
```

`apps/macos/` sits outside `crates/` so the Cargo workspace globs
(`crates/jp_*`, `crates/plugins/command/*`, `crates/contrib/*`) ignore it and
`cargo build` at the root stays a pure Rust build.
`crates/contrib/` is reserved for standalone Rust crates rather than Swift
targets.

### `Workspace::open`

The seven-step initialization sequence moves into `jp_workspace`, which already
owns `find_root`, `Id::load`, and `user_data_dir()`:

```rust
impl Workspace {
    /// Open the workspace containing `dir`, wiring filesystem and user-local
    /// storage.
    pub fn open(dir: &Utf8Path) -> Result<Self>;

    /// The filesystem storage backend, when opened from disk.
    pub fn fs_storage(&self) -> Option<&Arc<FsStorageBackend>>;
}
```

`jp_cli::load_workspace` retains its workspace-ID lookup branch and
`--no-persist` handling, and delegates the path branch to `Workspace::open`.
Behavior is unchanged, including the writes performed on open (creating the
user-local storage directory, repairing its symlink, and persisting the
workspace ID).

To make the in-memory constructors explicit, `new` and `new_with_id` are
renamed:

| Verb        | Method                                  | Consumer         |
| ----------- | --------------------------------------- | ---------------- |
| `in_memory` | `Workspace::in_memory{,_with_id}(root)` | tests            |
| `open`      | `Workspace::open(dir)`                  | CLI, app, future |
| (create)    | explicit builder, unchanged             | `jp init`        |

`jp init` creates a workspace where `.jp` does not yet exist and mints a new ID,
so it continues to build `Workspace` explicitly.
A `Workspace::create` constructor can be added if a second caller needs to
create workspaces.

The rename updates 106 call sites of `Workspace::new` and
`Workspace::new_with_id` across `crates/`, all but two of them in test code.

### FFI Boundary

`jp_ffi` compiles as a static library exposing six C-ABI functions:

```rust
pub extern "C" fn jp_workspace_open(path: *const c_char) -> *mut WorkspaceRef;
pub extern "C" fn jp_workspace_conversations(ws: *mut WorkspaceRef) -> *mut c_char;
pub extern "C" fn jp_workspace_events(ws: *mut WorkspaceRef, id: *const c_char) -> *mut c_char;
pub extern "C" fn jp_workspace_close(ws: *mut WorkspaceRef);
pub extern "C" fn jp_string_free(s: *mut c_char);
pub extern "C" fn jp_last_error() -> *mut c_char;
```

The boundary enforces five invariants:

- **No unwinding across FFI.** Every entry point wraps its body in
  `catch_unwind`, because a Rust panic unwinding across a C ABI boundary is
  undefined behavior.
- **Null return on failure.** Failed calls return null and store the error in
  thread-local storage, retrieved with `jp_last_error()`.
- **Owned data only.** Reads serialize their data and drop their `parking_lot`
  guard before returning.
  No guard, reference, or borrow crosses the boundary.
- **Rust frees what Rust allocates.** Swift frees returned strings by calling
  `jp_string_free`.
- **JSON payloads.** Data crosses the boundary as JSON and decodes into three
  hand-maintained Swift `Codable` structs.

`jp_ffi` depends on `jp_workspace` and `jp_conversation`, not `jp_config` or
`jp_cli`.
A read-only viewer does not apply per-tool styles, reasoning display modes, or
hidden-tool filters, so it needs neither `jp_config` nor the layered config
pipeline in `jp_cli`.

Conversation events already serialize to JSON for the plugin host, which answers
`PluginToHost::ReadEvents` with `Vec<Value>`.
`jp_workspace_events` reuses that projection.

### Build Integration

`just` runs the Rust build (`cargo build -p jp_ffi`), producing `libjp_ffi.a`
and a generated C header.
Xcode invokes `just` from a Run Script build phase and links `libjp_ffi.a`,
keeping `just` as the single build entry point.
Packaging an `.xcframework` is deferred until the app is distributed.

### `jp gui`

`jp-gui` is a command plugin under [RFD 072].
Because `InitMessage.workspace.root` already carries the resolved workspace
root, the plugin needs no path-resolution logic: it sends `Ready`, launches the
app with the root as an argument, and sends `Exit`.

`jp gui` opens the current directory's workspace, while the global
`-w`/`--workspace` flag (`jp -w ../other gui`) targets another workspace.
A trailing path (`jp gui .`) arrives in `init.args` and is validated against the
resolved root.

## Drawbacks

- **A second language in the repository.** CI requires a macOS runner for the
  Swift target, and contributors on other platforms cannot build the app.
  The Cargo workspace is unaffected, but a single command no longer runs every
  test in the repository.
- **No process isolation.** Linking Rust into the app process means an abort or
  memory corruption bug in `jp_workspace` crashes the app; `catch_unwind`
  catches panics, not aborts or undefined behavior.
- **Duplicated payload types.** Swift `Codable` structs mirror the shape of Rust
  types without compile-time checks that they match.
  Maintaining three types by hand is manageable initially, and the Alternatives
  section defines when to adopt a bindings generator.
- **Concrete backend exposure on `Workspace`.** Adding `fs_storage()` exposes
  `FsStorageBackend` on `Workspace` alongside its backend trait objects.
  Returning a tuple from `Workspace::open` would avoid the method, but forces
  every caller to thread both values.
- **Wide mechanical diff.** Renaming `Workspace::new` and
  `Workspace::new_with_id` touches 106 call sites, almost all in tests, in one
  commit.

## Alternatives

### Sidecar process speaking a protocol

The app could spawn a `jp` child process per window and communicate via
JSON-lines over pipes.
A sidecar provides process isolation and a protocol testable without Xcode, but
it requires process supervision, protocol versioning, and one child process per
window.
Nor can the existing plugin protocol be reused directly: in [RFD 072], `jp` is
the parent process that sends `Init`, whereas the app would need to be the
parent, inverting the roles.
For a read-only viewer with no long-running Rust state, a sidecar adds
complexity without a matching benefit.
Adding write support (such as sending a query from the app) would justify
revisiting this choice.

### Shelling out to `jp ... --format json`

Spawning `jp ... --format json` on each read is quick to prototype but relies on
an unstable contract.
`jp_cli::output` builds JSON from formatted table rows, so the app would bind to
presentation-derived shapes that are not stable, and Hyrum's Law would freeze
them once the app shipped.
It also pays a full workspace load on every read in a scrolling UI.

### UniFFI or swift-bridge from the start

Both generate typed Swift bindings and error mapping, eliminating the
hand-maintained `Codable` structs.
However, both add a codegen step to the build before the basic Xcode-to-Rust
link is proven.
Six functions and three payload types do not justify that build complexity.
We will adopt a bindings generator once the FFI surface exceeds roughly ten
functions or needs Rust-to-Swift callbacks for live updates.

### Swift reading `.jp/` directly

Rejected.
Reading `.jp/` in Swift reimplements the storage format and user-local merge
logic in a second language and freezes the on-disk layout as a public contract.

### A separate repository for the app

Rejected.
The FFI boundary changes on both sides at once, and a separate repository turns
every signature or payload change into a cross-repository coordination step.

## Non-Goals

- **Writing.** No compose field, editing, archiving, or config mutation.
  The app only opens and reads conversations.
- **Live filesystem watching.** Every FFI read reloads the conversation index,
  so a read triggered after a concurrent `jp query` sees that query's turns and
  any conversation it started.
  However, the app does not watch the store for changes; the view updates only
  when the user triggers a read.
  Push-based updates belong to the Live Workspace View draft.
- **Side-effect-free workspace opening.** `Workspace::open` creates the
  user-local storage directory, repairs its symlink, and persists the workspace
  ID, matching `jp`.
  Opening a workspace without disk writes is out of scope.
- **Renaming Project / Workspace / Storage.** `Workspace::open` uses the
  existing vocabulary.
  The Project, Workspace and Storage draft owns the terminology question.
- **Tool calls, attachments, and reasoning display.** Events other than user
  messages and assistant messages render as minimal placeholders.
- **iOS, iPadOS, Catalyst, App Store distribution, and sandboxing.**
- **Sharing the render projection.** The app is the third consumer that walks a
  `ConversationStream` for display, after `jp conversation print` and
  `serve-web`.
  Extracting a presentation-neutral projection is defensible at three consumers,
  but out of scope for this RFD.

## Risks and Open Questions

- **Xcode build integration.** Linking a Rust static library, generating
  headers, signing, and keeping incremental builds fast are the primary
  technical unknowns.
  Phase 2 isolates and validates the build pipeline early.
- **Recents without a document-based app.** Because a workspace is a directory
  rather than a file, SwiftUI's `DocumentGroup` is an awkward fit.
  We plan to use `WindowGroup` and `NSOpenPanel` with a manually managed recents
  list, and verify in Phase 2 whether `File > Open Recent` can be populated
  cleanly outside an `NSDocument` app.
- **Payload type reuse.** `jp_plugin::message::ConversationSummary` already has
  the fields the conversation list needs, and `jp_plugin` is a lightweight
  crate.
  Reusing it avoids a duplicate definition but couples the FFI payload to the
  plugin wire format, which may diverge later.
  We will reuse `ConversationSummary` for v0.1 and fork the type if the shapes
  diverge.
- **Concurrent reads while `jp query` writes.** The app reads files while
  another process may be appending to them.
  `serve-web` has the same access pattern with no reported corruption, but
  reading a partially written event file has not been deliberately tested.
- **Threading.** `Workspace` reads are synchronous and acquire locks.
  FFI calls must run off the main thread so a slow read does not block the UI.

## Implementation Plan

### Phase 1: `Workspace::open`

- Move the path branch of `jp_cli::load_workspace` into
  `jp_workspace::Workspace::open` and add `fs_storage()`.
- Rename `new` and `new_with_id` to `in_memory` and `in_memory_with_id`.
- Add a characterization test verifying that `Workspace::open` returns
  conversations stored only in user-local storage.
- Merges independently with no behavior change.

### Phase 2: FFI and Build Seam

- Create `crates/jp_ffi` with `jp_workspace_open`, `jp_workspace_conversations`,
  `jp_string_free`, and `jp_last_error`, each wrapped in `catch_unwind`.
- Set up the `apps/macos/` Xcode project, the `just` recipe for the static
  library, and the Run Script build phase.
- Render a single SwiftUI `List` showing conversation titles from a hardcoded
  workspace path to validate the build pipeline.
- Depends on Phase 1.

### Phase 3: Conversation Reader

- Add `jp_workspace_events` and `jp_workspace_close` to `jp_ffi`.
- Build the two-pane split view with the conversation list sorted by last
  activity and the history pane rendering user and assistant messages as native
  Markdown text.
- Implement `File > Open` with a directory chooser and workspace-root
  resolution.
- Delivers the MVP: open a JP workspace, and browse and scroll conversations.

### Phase 4: Native macOS Behavior and `jp gui`

- Define the affordance map covering menus, shortcuts, selection, copy
  representations, drag sources, and accessibility roles for the list and
  history panes.
- Implement window tabbing, per-window state restoration (split position,
  selection, scroll position), and `File > Open Recent`.
- Support double-clicking a conversation to open it in its own window.
- Run a manual QA checklist against the macOS behavior test plan.
- Add `crates/plugins/command/gui` implementing the [RFD 072] protocol to launch
  the app at `InitMessage.workspace.root` and exit.
  Included here rather than in its own phase because the plugin only requires an
  app that accepts a workspace path from Phase 3.

## References

- [RFD 072]: Command plugin protocol that `jp-gui` speaks.
- [RFD 073]: Layered workspace storage and why user-local storage must be wired
  before listing conversations.
- [Mac-arsed Mac App skill][mac-arsed]: Native macOS behavior bar for the app.

[RFD 072]: 072-command-plugin-system.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[mac-arsed]: https://github.com/bartreardon/skills/blob/main/mac-arsed-mac-app/SKILL.md
