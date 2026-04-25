# RFD D09: Project Filesystem Abstraction

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01

## Summary

This RFD introduces a `ProjectFiles` trait that abstracts read and write access
to the user's project files — the content that tools operate on and attachments
resolve against. `Workspace` holds a non-optional `Arc<dyn ProjectFiles>`,
replacing the current `root: Utf8PathBuf` field and eliminating the assumption
that project files live on a local filesystem. `FsProjectFiles` wraps the
current root-directory behavior. `NullProjectFiles` provides an empty
implementation for workspaces not tied to a project directory (tests, standalone
conversations). The trait includes a `materialize()` method that produces a
temporary filesystem directory from the backing store, enabling subprocess tools
to work against any backend. For `FsProjectFiles`, materialization returns the
real root path at zero cost.

## Motivation

`Workspace::root` is a `Utf8PathBuf` that points to the user's project
directory. It serves three purposes:

1. **Tool execution.** `run_tool_command()` uses `root` as `current_dir` for
   subprocess tools, and passes it in the tool context as `"root":
   root.as_str()`.
2. **Attachment resolution.** The `Handler::add` and `Handler::get` trait
   methods receive `cwd: &Utf8Path`, which is set to `workspace.root()`.
3. **Editor file placement.** The query editor writes `QUERY_MESSAGE.md` into
   a conversation directory derived from the storage path, with `root` as a
   fallback.

All three assume a local filesystem. This prevents JP from running in
environments without filesystem access — a browser using Web Storage, a
database-backed workspace, or a test harness that avoids temporary directories.

[RFD 073] removes `Workspace::root` as part of the storage backend
refactoring, replacing it with explicit `root` parameter threading for
callers that need the project path. That's a necessary intermediate step, but
it leaves the filesystem assumption intact: callers still pass a `Utf8Path`
that must point to a real directory.

The goal is to replace the path with a trait object. Callers that need project
file access go through `ProjectFiles`, which may be backed by a real directory,
an in-memory `HashMap`, or any future backend. Tools that execute as
subprocesses use `materialize()` to get a filesystem view they can work with.

### Concrete pain points

**Tests create temporary directories for project files.** Every integration
test that exercises tool execution or attachment resolution creates a
`tempdir` just so `root` has a valid path. An in-memory implementation would
eliminate this overhead and make tests faster, more deterministic, and
independent of filesystem state.

**Tool execution assumes `current_dir` exists.** `run_tool_command()` calls
`cmd.current_dir(root.as_std_path())`, which fails if the path doesn't exist.
In-memory workspaces (used in tests without storage) pass an empty
`Utf8PathBuf`, which is not a valid directory. This has led to workarounds
where test helper code creates directories just to satisfy the `current_dir`
requirement.

**Config loading hardcodes filesystem traversal.** The config pipeline in
`jp_cli` resolves `--cfg` paths by walking three filesystem roots:
user-global, workspace root, and user-workspace. This logic is tightly
coupled to `std::fs`. A future browser deployment would need an entirely
different config resolution strategy, but the current code has no seam for
this.

**Attachment handlers receive a raw path.** The `Handler` trait takes `cwd:
&Utf8Path` for path resolution. This works for local files but provides no
abstraction for handlers that need to resolve resources from non-filesystem
sources.

## Design

### The `ProjectFiles` Trait

`ProjectFiles` abstracts over the user's project files. It provides the
operations that tools, attachments, and config loading need:

```rust
pub trait ProjectFiles: Send + Sync + Debug {
    /// Read a file's contents.
    fn read(&self, path: &str) -> Result<Vec<u8>>;

    /// Write content to a file, creating parent directories as needed.
    fn write(&self, path: &str, content: &[u8]) -> Result<()>;

    /// Check whether a path exists.
    fn exists(&self, path: &str) -> bool;

    /// List entries in a directory.
    ///
    /// Returns relative paths from the given directory. Non-recursive.
    fn list_dir(&self, path: &str) -> Result<Vec<DirEntry>>;

    /// Get metadata for a path.
    fn metadata(&self, path: &str) -> Result<FileMetadata>;

    /// Delete a file.
    fn delete(&self, path: &str) -> Result<()>;

    /// Rename or move a file.
    fn rename(&self, from: &str, to: &str) -> Result<()>;

    /// Search file contents for a pattern.
    ///
    /// This is a first-class method rather than built from `list_dir` +
    /// `read` because implementations can optimize it significantly
    /// (e.g., using ripgrep on a real filesystem, or indexed search on a
    /// database backend).
    fn grep(&self, pattern: &str, opts: GrepOpts) -> Result<Vec<GrepMatch>>;

    /// Produce a temporary filesystem directory backed by this store.
    ///
    /// Subprocess tools that assume direct filesystem access use this to
    /// get a real directory they can `current_dir` into and read/write
    /// files from.
    ///
    /// For `FsProjectFiles`, this returns the real root path — no copy,
    /// no temporary directory, zero cost. For non-filesystem backends,
    /// this creates a temporary directory, populates it from the backing
    /// store, and returns a guard that syncs changes back on drop.
    fn materialize(&self) -> Result<MaterializedView>;

    /// A display-friendly identifier for the project root.
    ///
    /// For filesystem backends, this is the absolute path. For other
    /// backends, it could be a URL, a database identifier, or a
    /// descriptive label.
    fn display_root(&self) -> &str;
}
```

Paths are `&str`, not `&Utf8Path` or `&Path`. This is deliberate: the trait
is backend-agnostic, and paths are treated as opaque identifiers that look
like relative filesystem paths. The `FsProjectFiles` implementation resolves
them against the real root directory. Other implementations interpret them
however they need to.

All paths are relative to the project root. Absolute paths and paths
containing `..` that escape the root are rejected by all implementations.

### `DirEntry` and `FileMetadata`

```rust
#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Relative path from the listed directory.
    pub path: String,

    /// Whether this entry is a file or directory.
    pub kind: EntryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
}

#[derive(Debug, Clone)]
pub struct FileMetadata {
    pub kind: EntryKind,
    pub size: u64,
}
```

These types intentionally mirror `jp:host/filesystem` from [RFD 016]. When
the WASM plugin architecture is implemented, the `jp:host/filesystem` host
imports delegate directly to `ProjectFiles` — the logical operations are the
same, only the transport differs.

### `GrepOpts` and `GrepMatch`

```rust
#[derive(Debug, Clone, Default)]
pub struct GrepOpts {
    /// File extensions to search (empty = all files).
    pub extensions: Vec<String>,

    /// Directories to restrict the search to (empty = entire project).
    pub paths: Vec<String>,

    /// Number of context lines before and after matches.
    pub context: usize,
}

#[derive(Debug, Clone)]
pub struct GrepMatch {
    /// The file path relative to the project root.
    pub path: String,

    /// Matching lines with optional context.
    pub lines: Vec<MatchLine>,
}

#[derive(Debug, Clone)]
pub struct MatchLine {
    /// 1-based line number.
    pub line_number: usize,

    /// The line content.
    pub content: String,

    /// Whether this line is a match or a context line.
    pub is_match: bool,
}
```

`grep` is a first-class trait method because the performance difference
between "grep via the filesystem" and "read every file through `read()` and
search in memory" is orders of magnitude. On a real filesystem,
`FsProjectFiles::grep` can delegate to an optimized search (ripgrep-style
parallel directory walking with memory-mapped files). Other backends can use
indexed search or database queries. Building grep from `list_dir` + `read`
would force the worst-case implementation on every backend.

### `MaterializedView`

```rust
/// A temporary filesystem directory backed by a `ProjectFiles` store.
///
/// For filesystem-backed projects, this is a zero-cost wrapper around the
/// real root path. For other backends, this is a temporary directory that
/// was populated from the backing store.
///
/// When the view is dropped, modified files are synced back to the backing
/// store (for non-filesystem backends). For filesystem backends, drop is
/// a no-op — files were modified in place.
pub struct MaterializedView {
    root: Utf8PathBuf,
    sync_back: Option<Box<dyn FnOnce(&Utf8Path) -> Result<()> + Send>>,
}

impl MaterializedView {
    /// The filesystem path tools can use as `current_dir`.
    pub fn path(&self) -> &Utf8Path {
        &self.root
    }
}

impl Drop for MaterializedView {
    fn drop(&mut self) {
        if let Some(sync) = self.sync_back.take() {
            if let Err(e) = sync(&self.root) {
                tracing::error!(%e, "Failed to sync materialized view back to store");
            }
        }
    }
}
```

For `FsProjectFiles`, `materialize()` returns a `MaterializedView` with
`sync_back: None` — it points to the real directory and drop does nothing.
Zero allocation, zero copy.

For `InMemoryProjectFiles`, `materialize()` creates a temporary directory,
writes all stored files to it, and sets `sync_back` to a closure that reads
back modified files on drop. This is more expensive but only happens for
non-filesystem backends.

### `FsProjectFiles`

The filesystem implementation wraps a root path:

```rust
#[derive(Debug, Clone)]
pub struct FsProjectFiles {
    root: Utf8PathBuf,
}

impl FsProjectFiles {
    pub fn new(root: impl Into<Utf8PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl ProjectFiles for FsProjectFiles {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        let full = self.resolve(path)?;
        std::fs::read(&full).map_err(Into::into)
    }

    fn write(&self, path: &str, content: &[u8]) -> Result<()> {
        let full = self.resolve(path)?;
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&full, content).map_err(Into::into)
    }

    fn materialize(&self) -> Result<MaterializedView> {
        Ok(MaterializedView {
            root: self.root.clone(),
            sync_back: None,
        })
    }

    fn display_root(&self) -> &str {
        self.root.as_str()
    }

    // ... remaining methods delegate to std::fs with path resolution
}
```

`resolve()` is an internal method that joins the relative path to `root` and
validates the result doesn't escape the root via `..` traversal. This
provides a basic safety guarantee even for the filesystem backend.

### `NullProjectFiles`

A minimal implementation for workspaces not tied to a project directory:

```rust
#[derive(Debug, Clone, Default)]
pub struct NullProjectFiles;

impl ProjectFiles for NullProjectFiles {
    fn read(&self, path: &str) -> Result<Vec<u8>> {
        Err(Error::NoProjectFiles(path.to_owned()))
    }

    fn write(&self, path: &str, _: &[u8]) -> Result<()> {
        Err(Error::NoProjectFiles(path.to_owned()))
    }

    fn exists(&self, _path: &str) -> bool {
        false
    }

    fn list_dir(&self, _path: &str) -> Result<Vec<DirEntry>> {
        Ok(vec![])
    }

    fn grep(&self, _: &str, _: GrepOpts) -> Result<Vec<GrepMatch>> {
        Ok(vec![])
    }

    fn materialize(&self) -> Result<MaterializedView> {
        Err(Error::NoProjectFiles("materialize".to_owned()))
    }

    fn display_root(&self) -> &str {
        "<no project>"
    }

    // ... remaining methods return errors or empty results
}
```

This replaces the current pattern where `Workspace::new(Utf8PathBuf::new())`
creates a workspace with an empty root path. With `NullProjectFiles`, the
type system makes it explicit that there are no project files rather than
having an empty path that silently fails when used as `current_dir`.

### `InMemoryProjectFiles`

A `HashMap`-backed implementation for tests and non-filesystem environments:

```rust
#[derive(Debug, Clone, Default)]
pub struct InMemoryProjectFiles {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    label: String,
}
```

`read`, `write`, `exists`, `list_dir`, `delete`, and `rename` operate on the
in-memory map. `grep` does a linear scan over stored files. `materialize()`
creates a temporary directory, writes all entries, and sets up a sync-back
closure.

This is primarily useful for tests. A browser-based `WebStorageProjectFiles`
or database-backed implementation would follow the same pattern but use
different backing stores.

### `Workspace` Integration

After [RFD 073] removes `Workspace::root`, `Workspace` gains a
`ProjectFiles` field:

```rust
pub struct Workspace {
    id: Id,
    project: Arc<dyn ProjectFiles>,
    persist: Arc<dyn PersistBackend>,
    loader: Arc<dyn LoadBackend>,
    locker: Arc<dyn LockBackend>,
    sessions: Arc<dyn SessionBackend>,
    state: State,
}
```

`project` is non-optional. Callers access project files through
`workspace.project()`, which returns `&Arc<dyn ProjectFiles>`.

Construction for a filesystem-backed workspace:

```rust
let project = FsProjectFiles::new(&workspace_root);
let backend = FsStorageBackend::new(&storage_path)?
    .with_user_storage(&user_root, name, id)?;

let workspace = Workspace::new(ws_id, Arc::new(project), &backend);
```

Construction for tests:

```rust
let workspace = Workspace::new(
    ws_id,
    Arc::new(NullProjectFiles),
    &InMemoryStorageBackend::default(),
);
```

Or with in-memory project files that tools can read/write:

```rust
let mut project = InMemoryProjectFiles::default();
project.write("src/main.rs", b"fn main() {}")?;

let workspace = Workspace::new(
    ws_id,
    Arc::new(project),
    &InMemoryStorageBackend::default(),
);
```

### Caller Migration

Each current use of `workspace.root()` migrates to `ProjectFiles`:

#### Tool execution

Currently `run_tool_command()` receives `root: &Utf8Path` and calls
`cmd.current_dir(root)`. After this RFD, it receives `project: &Arc<dyn
ProjectFiles>`, calls `project.materialize()`, and uses the materialized
path as `current_dir`:

```rust
let view = project.materialize()?;
cmd.current_dir(view.path().as_std_path());
```

The tool context JSON changes from `"root": root.as_str()` to `"root":
view.path().as_str()`. From the tool's perspective, nothing changes — it
still sees a filesystem path as `current_dir` and in the context.

#### Attachment resolution

The `Handler` trait currently takes `cwd: &Utf8Path`. This changes to `cwd:
&Arc<dyn ProjectFiles>`. Handlers that resolve local files call
`cwd.read(path)` or use `cwd.materialize()` if they need a real filesystem
path (e.g., for spawning subprocesses). The `file` attachment handler is the
primary consumer — it reads file content through `ProjectFiles::read()`
instead of `std::fs::read()`.

#### Editor file placement

The query editor writes `QUERY_MESSAGE.md` to a conversation directory
derived from the storage path. This is a storage concern, not a project
files concern — the editor file lives in `.jp/conversations/`, not in the
user's code. After [RFD 073], this path comes from the storage backend, not
from `workspace.root()`. No change needed from this RFD.

#### Config loading

The config pipeline resolves `--cfg` paths against three roots: user-global,
workspace root, and user-workspace. The workspace root component currently
uses `workspace.root()`. After this RFD, it uses
`project.display_root()` for logging and `FsProjectFiles`-specific path
access for actual file resolution.

Config loading is not fully abstracted by this RFD. The filesystem traversal
logic remains in `jp_cli`'s config pipeline. Abstracting config loading for
non-filesystem backends (e.g., a `ConfigSource` trait) is future work — it
depends on having concrete requirements from a non-filesystem deployment.
This RFD replaces the `workspace.root()` call with `project` access, but
the config pipeline still assumes the project root is a filesystem directory
when resolving relative config paths.

### Relationship to `jp:host/filesystem` (RFD 016)

The `ProjectFiles` trait and the `jp:host/filesystem` WIT interface from
[RFD 016] define the same logical operations: `read`, `write`, `list_dir`,
`metadata`. This is intentional. When the WASM plugin architecture is
implemented, the host-side implementation of `jp:host/filesystem` delegates
to the `ProjectFiles` trait object on the workspace. The WIT interface is the
guest-facing contract; `ProjectFiles` is the host-side abstraction.

The same relationship holds for future VFS-mediated subprocess tools: the
stdio IPC protocol exposes `read`, `write`, `list_dir`, etc., and the host
resolves each request through `ProjectFiles`.

```text
                      ProjectFiles (trait)
                     ┌────────┴────────┐
            FsProjectFiles      InMemoryProjectFiles
                     │
        ┌────────────┼────────────┐
        │            │            │
   Subprocess    WASM host     Stdio IPC
   (materialize) (jp:host/fs)  (JSON-RPC)
```

## Drawbacks

**Trait overhead for the common case.** In production, JP always has a real
filesystem. Every `ProjectFiles` call goes through dynamic dispatch where
`std::fs` would suffice. The cost is a vtable lookup per call — negligible
at I/O boundaries, but it's added indirection for the dominant use case to
support a minority case (non-filesystem backends).

**`materialize()` is a compatibility escape hatch.** The entire point of
`ProjectFiles` is to abstract away the filesystem, but `materialize()` says
"give me a real filesystem anyway." This is necessary for subprocess tools
that assume direct file access, but it means non-filesystem backends pay a
materialization cost (temp dir creation, file population, sync-back on drop)
that filesystem backends don't. The cost is proportional to project size and
could be significant for large projects.

**`grep` as a trait method is opinionated.** Most VFS abstractions don't
include search. Including it couples the trait to a specific operation that
not all backends can implement efficiently. However, `grep` is JP's
most-used tool operation by a wide margin — building it from `list_dir` +
`read` would be unacceptably slow on any backend, so every implementation
needs an optimized path regardless.

**Attachment handlers need updating.** The `Handler` trait's `cwd: &Utf8Path`
parameter changes to `&Arc<dyn ProjectFiles>`, which is a breaking change to
the attachment handler interface. All handlers (`file_content`,
`cmd_output`, `bear_note`, `http_content`, `mcp_resources`) need updating,
though most don't use `cwd` for filesystem access.

## Alternatives

### Keep `root` as `Option<Utf8PathBuf>`

After [RFD 073] removes `root` from `Workspace`, thread it as an optional
parameter through callers. No trait, no abstraction — callers that need a
filesystem path get `Some(path)` or `None`.

Rejected because it reintroduces the optionality pattern that [RFD 073]
eliminates for storage. Every caller would need to branch on `root.is_some()`,
which is the same problem. Polymorphism (a trait with `NullProjectFiles`) is
the established pattern from the storage refactoring.

### VFS crate from the ecosystem

Use an existing Rust VFS crate (e.g., `vfs`, `async-vfs`) instead of a
custom trait. These crates provide filesystem abstraction with in-memory and
real-filesystem backends.

Rejected because JP's requirements are narrow and specific. We need `grep`
as a first-class operation, `materialize()` for subprocess compatibility,
and alignment with the `jp:host/filesystem` WIT interface. An off-the-shelf
VFS would require wrapping, adapting, and extending to the point where the
custom trait is simpler. The trait surface is small (~10 methods) and
unlikely to grow much.

### Combine `ProjectFiles` with storage backends

Make `ProjectFiles` part of the storage backend from [RFD 073]. The workspace
would hold one fewer trait object, and filesystem-backed workspaces could
share a single struct for both storage and project files.

Rejected because project files and storage serve different purposes with
different access patterns. Project files are the user's code — read by tools,
referenced by attachments, grepped for content. Storage is JP's internal
data — conversations, sessions, locks. Coupling them means a mock that only
needs to test tool execution must also implement conversation persistence,
and vice versa.

## Non-Goals

- **Config loading abstraction.** The config pipeline continues to resolve
  paths against filesystem directories for now. A `ConfigSource` trait for
  non-filesystem config loading is future work.

- **Cross-project file access.** `ProjectFiles` is scoped to a single
  project. Accessing files outside the project root (e.g., `~/Downloads/`)
  is an attachment concern handled at the CLI layer, not a VFS concern.

- **Write-back policies.** `MaterializedView` syncs all changes back on drop.
  More sophisticated policies (selective sync, conflict resolution, dry-run
  mode) are future work.

- **Non-filesystem backends beyond `InMemoryProjectFiles`.** This RFD provides
  `FsProjectFiles`, `NullProjectFiles`, and `InMemoryProjectFiles`. Browser,
  database, or cloud storage backends are future work that will implement the
  same trait.

## Risks and Open Questions

### `materialize()` scope and lifetime

For filesystem backends, `materialize()` returns a view that lives as long as
the `MaterializedView` guard. For non-filesystem backends, the temporary
directory persists until the guard drops and syncs back. If a tool execution
takes a long time (minutes), the temporary directory consumes disk space for
the duration. This is acceptable for the expected use case (tools run for
seconds, not minutes) but worth noting.

### Partial materialization

The current design materializes the entire project. For large projects with
non-filesystem backends, this could be prohibitively expensive. A future
optimization could materialize only the files the tool needs, using the tool's
declared file access patterns or lazy population via FUSE. This is out of
scope for this RFD but noted as a potential future need.

### `grep` implementation for non-filesystem backends

`InMemoryProjectFiles` implements `grep` as a linear scan over all stored
files with a compiled regex. This is adequate for tests but may not scale for
production non-filesystem backends with large file sets. Database-backed
implementations could use full-text search indexes. The trait intentionally
doesn't prescribe the implementation strategy.

### Attachment handler migration

The `Handler` trait change from `cwd: &Utf8Path` to `cwd: &Arc<dyn
ProjectFiles>` is a breaking change. Several handlers (`bear_note`,
`http_content`, `mcp_resources`) don't use `cwd` at all — they fetch content
from external sources. Only `file_content` and `cmd_output` use `cwd` for
local file resolution. The migration is straightforward but touches multiple
crates.

## Implementation Plan

### Phase 1: Define trait and types in a new `jp_project` crate

Add the `ProjectFiles` trait, `DirEntry`, `FileMetadata`, `GrepOpts`,
`GrepMatch`, `MaterializedView`, and the error type. No implementations yet.

The new crate is named `jp_project` rather than extending `jp_storage`,
because project files and JP storage are distinct concerns.

**Depends on:** Nothing.
**Mergeable:** Yes.

### Phase 2: `FsProjectFiles`

Implement `ProjectFiles` for the filesystem backend. Path validation
(reject absolute paths, `..` traversal). Delegate to `std::fs` for all
operations. `materialize()` returns the real root with no sync-back.
`grep` uses a parallel directory walker with compiled regex.

**Depends on:** Phase 1.
**Mergeable:** Yes.

### Phase 3: `NullProjectFiles` and `InMemoryProjectFiles`

Implement both. Add tests exercising the same operations against all three
implementations to verify behavioral equivalence (where applicable —
`NullProjectFiles` returns errors, which is the expected behavior).

**Depends on:** Phase 1.
**Mergeable:** Yes (parallel with Phase 2).

### Phase 4: Add `ProjectFiles` to `Workspace`

Add `project: Arc<dyn ProjectFiles>` to `Workspace`. Expose
`workspace.project()`. Update `Workspace` constructors to accept a
`ProjectFiles` implementation.

**Depends on:** [RFD 073] Phase 4 (Workspace refactor removing `root`),
Phase 2, Phase 3.
**Mergeable:** Yes.

### Phase 5: Migrate tool execution

Update `run_tool_command()` and the tool execution pipeline to receive
`ProjectFiles` instead of `root: &Utf8Path`. Use `materialize()` for
subprocess `current_dir`. Update the tool context JSON.

**Depends on:** Phase 4.
**Mergeable:** Yes.

### Phase 6: Migrate attachment handlers

Update the `Handler` trait to accept `&Arc<dyn ProjectFiles>` instead of
`cwd: &Utf8Path`. Update all handler implementations. `file_content` uses
`ProjectFiles::read()`. `cmd_output` uses `materialize()` for subprocess
execution.

**Depends on:** Phase 4.
**Mergeable:** Yes (parallel with Phase 5).

### Phase 7: Migrate config pipeline

Replace `workspace.root()` usage in the config pipeline with `ProjectFiles`
access. The filesystem traversal logic remains, but the root path comes from
`FsProjectFiles` rather than `Workspace`.

**Depends on:** Phase 4.
**Mergeable:** Yes (parallel with Phases 5 and 6).

## References

- [RFD 016] — Wasm Plugin Architecture. Defines `jp:host/filesystem` with
  the same logical operations as `ProjectFiles`.
- [RFD 073] — Layered Storage Backend for Workspaces. Removes
  `Workspace::root` and introduces storage backend traits, which this RFD
  complements with a project file abstraction.

[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
