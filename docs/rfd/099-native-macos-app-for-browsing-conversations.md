# RFD 099: Native macOS App for Browsing Conversations

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31

## Summary

This RFD introduces a native macOS app for reading JP conversations, built in
SwiftUI over a thin Rust FFI crate (`jp_ffi`) that links `jp_workspace` and
`jp_conversation` directly into the app process.
It adds `Workspace::open` so that "turn a path into a usable workspace" becomes
a library capability instead of private CLI code, and ships a `jp-gui` command
plugin so `jp gui` opens the current workspace in the app.

## Motivation

JP conversations are readable from the terminal (`jp conversation print`) and,
with [RFD 072]'s plugin system, from a browser (`jp serve web`).
Neither is a good fit for browsing: scrolling back through weeks of turns,
skimming several conversations side by side, or reading on the machine where the
work happened.
That is what a Mac app is for, and it should be a real one: native windows,
tabs, menus, keyboard, and state restoration, not a web view in a window.

The second motivation is structural.
`Workspace` has had exactly one consumer since it was written, and its API shows
it.
The sequence that turns a filesystem path into a usable workspace is seven steps
long and lives in `load_workspace`, a private function in `jp_cli`:

1. Walk up from a directory looking for `.jp` (`Workspace::find_root`).
2. Load the workspace `Id` from `.jp/`.
3. Build `FsStorageBackend::new(.jp)`.
4. Resolve `user_data_dir()/workspace` as the user-storage root.
5. Wire user storage with `with_user_storage(user_root, slug, id)`.
6. `Workspace::new_with_id(root, id).with_backend(fs)`.
7. Persist the ID back with `id().store(&storage)`.

Nothing outside `jp_cli` can call that.
A second consumer has three options: depend on `jp_cli` (which pulls in clap,
the printer, every LLM provider, and MCP), copy the forty lines, or extract
them.

Copying is worse than it looks because of step 5.
Skip it and everything compiles, `Workspace::conversations()` still returns
results, and no error fires.
You see a subset, because conversations are read from the user-local silo as
well as `.jp/` (see [RFD 073]).
RFD 072 records this already happening once, when the web server was written
without local storage wired up.

Meanwhile `Workspace::new(path)` is public, is the most inviting door in the
API, and is the wrong one: it hands back an in-memory workspace with no
filesystem backend.
It exists so tests can build a workspace without touching disk, and to leave
room for future designs that drop the user-storage requirement.
Both are good reasons for the constructor to exist.
Neither is a reason for it to be the one a new consumer finds first.

At N=1 consumer this cost was invisible. At N=2 it is the first thing the second
consumer hits.

## Design

### User Experience

The app is a viewer: it opens workspaces and reads conversations.
It never writes conversation data and has no compose field.

- **Opening.** `File > Open` presents a directory chooser; picking any directory
  inside a workspace opens that workspace, matching `jp`'s own behavior of
  walking up to find `.jp`.
  `File > Open Recent` lists previously opened workspaces.
- **Windows.** One window per workspace, participating in native window tabbing,
  so tabs can be pulled out into their own windows.
- **Layout.** Two panes: conversation list on the left, conversation history on
  the right, split by a draggable divider whose position is restored across
  launches.
- **Conversations.** The list sorts by last activity and shows title and
  timestamp.
  Double-clicking a conversation opens it in its own window.
- **Terminal integration.** `jp gui` opens the current workspace in the app.

Menus, keyboard shortcuts, selection behavior, copy representations, and drag
sources are designed as their own deliverable (see the Implementation Plan);
this RFD fixes the shape of the app, not its affordance map.

### Repository Layout

```txt
crates/jp_ffi/                 # staticlib: C ABI over jp_workspace + jp_conversation
crates/plugins/command/gui/    # jp-gui: launches the app at a workspace root
apps/macos/                    # Xcode project and Swift sources (not a cargo member)
```

`apps/macos/` sits outside `crates/` so the cargo workspace globs
(`crates/jp_*`, `crates/plugins/command/*`, `crates/contrib/*`) are unaffected
and `cargo build` at the root stays a pure Rust build.
`crates/contrib/` is for standalone Rust crates and is a poor fit for a Swift
target.

### `Workspace::open`

The seven-step sequence moves into `jp_workspace`, which already owns
`find_root`, `Id::load`, and `user_data_dir()`:

```rust
impl Workspace {
    /// Open the workspace containing `dir`, wiring filesystem and user-local
    /// storage.
    pub fn open(dir: &Utf8Path) -> Result<Self>;

    /// The filesystem storage backend, when opened from disk.
    pub fn fs_storage(&self) -> Option<&Arc<FsStorageBackend>>;
}
```

`jp_cli::load_workspace` keeps its workspace-ID lookup branch and `--no-persist`
handling, and delegates the path case to `Workspace::open`.
Behavior is identical, including the writes on open (silo creation, symlink
repair, ID persistence).

To stop the wrong door being the obvious one, the in-memory constructors are
renamed to say what they are:

| Verb        | Method                                  | Consumer         |
| ----------- | --------------------------------------- | ---------------- |
| `in_memory` | `Workspace::in_memory{,_with_id}(root)` | tests            |
| `open`      | `Workspace::open(dir)`                  | CLI, app, future |
| (create)    | explicit builder, unchanged             | `jp init`        |

`jp init` is creating rather than opening (there is no `.jp` yet and it mints
the ID), so it keeps building explicitly.
A `Workspace::create` verb can be introduced when a second creator exists.

The rename touches 106 call sites of `Workspace::new` / `Workspace::new_with_id`
across `crates/`, all but two of them in test code.
It is mechanical, and it is the moment to do it: the API is about to acquire its
second consumer.

### The FFI Boundary

`jp_ffi` compiles as a static library with a C ABI.
Six entry points:

```rust
pub extern "C" fn jp_workspace_open(path: *const c_char) -> *mut WorkspaceRef;
pub extern "C" fn jp_workspace_conversations(ws: *mut WorkspaceRef) -> *mut c_char;
pub extern "C" fn jp_workspace_events(ws: *mut WorkspaceRef, id: *const c_char) -> *mut c_char;
pub extern "C" fn jp_workspace_close(ws: *mut WorkspaceRef);
pub extern "C" fn jp_string_free(s: *mut c_char);
pub extern "C" fn jp_last_error() -> *mut c_char;
```

Rules the boundary holds to:

- **Every entry point wraps its body in `catch_unwind`.** A panic unwinding
  across the FFI boundary is undefined behavior.
  This wrapper is what makes an in-process boundary acceptable rather than
  reckless.
- **Failures return null and set a thread-local error**, retrieved with
  `jp_last_error()`.
- **Owned data only.** Reads copy out and drop their `parking_lot` guard before
  returning.
  No guard, reference, or borrow crosses the boundary.
- **Rust frees what Rust allocates.** Swift calls `jp_string_free`.
- **JSON payloads, decoded into Swift `Codable` structs.** Three payload types
  hand-maintained on the Swift side.

`jp_ffi` depends on `jp_workspace` and `jp_conversation`.
Not `jp_config`, not `jp_cli`.
A reader needs no config: there is no per-tool style to apply, no reasoning
display mode, no hidden-tool filtering.
Keeping `jp_config` out keeps the layered load pipeline (which lives in
`jp_cli`) out with it.

Events already serialize to JSON: the plugin host answers
`PluginToHost::ReadEvents` with `Vec<Value>` today, so `jp_workspace_events` is
the same projection behind a different door.

### Build Integration

`just` owns the Rust build (`cargo build -p jp_ffi`), producing `libjp_ffi.a`
and a generated header.
Xcode invokes `just` from a Run Script build phase and links the artifact, so
there is one build entry point rather than two competing ones.
An `.xcframework` is only worth assembling when the app is distributed.

### `jp gui`

A command plugin under [RFD 072].
`InitMessage.workspace.root` already carries a resolved workspace root, so the
plugin needs no path logic: it sends `Ready`, launches the app with the root as
an argument, and sends `Exit`.

`jp gui` uses the current directory's workspace; `jp -w ../other gui` targets
another one, since `-w/--workspace` is already a global flag.
A trailing path (`jp gui .`) arrives in `init.args` and is validated against the
resolved root.

## Drawbacks

- **A second language in the repository.** CI needs a macOS runner for the Swift
  target, and contributors on other platforms cannot build the app.
  The Rust workspace is unaffected, but "run the tests" stops being one command.
- **No failure isolation.** A bug in `jp_workspace` crashes the app.
  The `catch_unwind` wrapper converts panics into errors; it does nothing about
  aborts or memory corruption.
- **A hand-maintained mirror.** Swift `Codable` structs duplicate the shape of
  Rust types with no compiler checking that they match.
  Cheap at three types, and the reason the Alternatives section names a trigger
  for adopting a bindings generator.
- **`Workspace` grows a second public shape.** `fs_storage()` means `Workspace`
  knows one concrete backend type alongside its trait objects.
  The alternative is returning a tuple from `open` and threading it through
  every caller, which is worse at more call sites.
- **A rename with a wide diff.** 106 call sites, almost all tests, in one
  mechanical commit.

## Alternatives

### Sidecar process speaking a protocol

The app spawns a `jp` child per window and talks JSON-lines over pipes.
This buys real failure isolation and a protocol testable from both sides without
Xcode.
It costs process supervision, protocol versioning, and one child process per
window, and the existing plugin protocol cannot be reused as-is: `jp` is the
parent that issues `Init`, and the app needs to be the parent, so the roles
invert.
For a read-only viewer with no long-running Rust state, that is complexity
without a matching benefit.
Write support (sending a query from the app) is the condition that flips this
call.

### Shelling out to `jp ... --format json`

Fastest to start and the worst contract.
`jp_cli::output` builds JSON from table rows, so the app would bind to
presentation-derived shapes that are explicitly not stable yet, and Hyrum's Law
would freeze them the moment the app shipped.
It also pays a full workspace load per read, in a UI that scrolls.

### UniFFI or swift-bridge from the start

Both generate typed Swift bindings and error mapping, removing the
hand-maintained mirror.
Both also add a codegen step to the build at the moment when the build is the
risky part.
Six functions and three payload types do not justify it.
The trigger to adopt one: the surface passing roughly ten calls, or needing
Rust-to-Swift callbacks for live updates.

### Swift reading `.jp/` directly

Rejected.
It reimplements the storage format in a second language and freezes the on-disk
layout as a public contract.

### A separate repository for the app

Rejected.
The FFI boundary changes on both sides at once, and a split repository turns
every signature change into a two-repository dance.

## Non-Goals

- **Writing.** No compose field, no editing, no archiving, no config mutation.
  The app opens conversations and reads them.
- **Live updates.** A window loads its workspace once and does not refresh.
  Turns added by a concurrent `jp query` are invisible until the window is
  reopened.
  This is v0.1 behavior by decision, not an oversight; the Live Workspace View
  draft is the mechanism for fixing it properly.
- **Read-only workspace opening.** Opening a workspace creates the user-local
  silo, repairs its symlink, and persists the workspace ID, exactly as `jp` does
  today.
  Making that side-effect-free is out of scope.
- **Renaming Project / Workspace / Storage.** `Workspace::open` is a new door on
  the existing vocabulary.
  The Project, Workspace and Storage draft owns the terminology question.
- **Tool calls, attachments, and reasoning display.** Events other than user
  messages and assistant messages render as a minimal placeholder.
- **iOS, iPadOS, Catalyst, App Store distribution, sandboxing.**
- **Sharing the render projection.** The app is the third consumer that walks a
  `ConversationStream` to decide what to show, after terminal print and
  serve-web.
  Extracting a presentation-neutral projection is defensible at three consumers,
  but not in this RFD.

## Risks and Open Questions

- **Xcode build integration is the real risk, not SwiftUI.** Linking a Rust
  static library, header generation, signing, and keeping the inner loop fast
  are all unknowns until tried.
  Phase 2 exists to find out early.
- **Recents without a document-based app.** A workspace is a directory, not a
  file, which makes SwiftUI's `DocumentGroup` an awkward fit.
  The lean is `WindowGroup` plus `NSOpenPanel` with a hand-managed recents list,
  but whether `File > Open Recent` can be populated cleanly outside an
  `NSDocument` app needs verifying in Phase 2.
- **Payload type reuse.** `jp_plugin::message::ConversationSummary` already has
  exactly the fields the list needs, and `jp_plugin` is a light crate.
  Reusing it saves a definition but couples the FFI payload to the plugin wire
  format, which will eventually diverge.
  The lean is to reuse it for v0.1 and fork when the shapes disagree.
- **Concurrent reads while `jp query` writes.** The app reads files another
  process appends to.
  `serve-web` has the same exposure and no reported corruption, but a partially
  written event file has not been deliberately tested.
- **Threading.** `Workspace` reads are synchronous and take locks.
  FFI calls must run off the main thread, or the UI stalls behind a lock held by
  a slow read.

## Implementation Plan

### Phase 1: `Workspace::open`

- Move the path branch of `jp_cli::load_workspace` into
  `jp_workspace::Workspace::open`; add `fs_storage()`.
- Rename `new` / `new_with_id` to `in_memory` / `in_memory_with_id`.
- Characterization test first: opening a workspace whose conversations exist
  only in the user-local silo returns them.
- No behavior change.
  Merges independently, and pays for itself in `jp_cli` alone.

### Phase 2: The seam

- `crates/jp_ffi` with `jp_workspace_open`, `jp_workspace_conversations`,
  `jp_string_free`, `jp_last_error`, each wrapped in `catch_unwind`.
- `apps/macos/` Xcode project, `just` recipe for the static library, Run Script
  build phase.
- One SwiftUI `List` showing conversation titles from a hardcoded workspace
  path.
- Nothing else.
  This phase exists to prove the build, not the app.
- Depends on Phase 1.

### Phase 3: The reader

- `jp_workspace_events`, `jp_workspace_close`.
- Two-pane split view, conversation list sorted by last activity, history pane
  rendering user and assistant messages as native text with markdown.
- `File > Open` with a directory chooser and workspace-root resolution.
- This phase satisfies the MVP goal: open the JP workspace, browse and scroll
  conversations.

### Phase 4: Mac behavior and `jp gui`

- Affordance map covering menus, shortcuts, selection, copy representations,
  drag sources, and accessibility roles for the list and history panes.
- Window tabbing, per-window state restoration (split position, selection,
  scroll position), `File > Open Recent`.
- Double-click to open a conversation in its own window.
- Manual QA checklist against the Mac behavior test plan.
- `crates/plugins/command/gui` implementing the [RFD 072] protocol, launching the
  app at `InitMessage.workspace.root` and exiting.
  Folded in here rather than given a phase of its own: it is one small plugin
  that needs only an app accepting a workspace path, which Phase 3 delivers.

## References

- [RFD 072] for the command plugin protocol that `jp-gui` speaks.
- [RFD 073] for why user-local storage must be wired before conversations can be
  listed.
- [Mac-arsed Mac App skill][mac-arsed] for the native-behavior bar the app is
  held to.

[RFD 072]: 072-command-plugin-system.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[mac-arsed]: https://github.com/bartreardon/skills/blob/main/mac-arsed-mac-app/SKILL.md
