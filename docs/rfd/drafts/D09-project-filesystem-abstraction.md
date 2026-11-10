# RFD D09: Project Filesystem Abstraction

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01
- **Required by**: [RFD D11]
- **Summary**: Replaces `Workspace::root` with a `ProjectFiles` trait to
  abstract project file access across filesystem and in-memory backends.

## Summary

This RFD replaces `Workspace`'s `root: Utf8PathBuf` field with a non-optional
`Arc<dyn ProjectFiles>` trait object that abstracts read and write access to
project files for tools and attachments.
`FsProjectFiles` wraps a local directory, while `NullProjectFiles` and
`InMemoryProjectFiles` support tests and non-filesystem environments.
For subprocess tools that require a filesystem path, `materialize()` returns a
temporary directory view on non-filesystem backends and the real root path at
zero cost on `FsProjectFiles`.

## Motivation

`Workspace::root` is a `Utf8PathBuf` pointing to the user's project directory,
used in three places:

1. **Tool execution.** `run_tool_command()` sets `current_dir` to `root` for
   subprocess tools and passes `"root": root.as_str()` in the tool context.
2. **Attachment resolution.** `Handler::add` and `Handler::get` receive `cwd:
   &Utf8Path`, set to `workspace.root()`.
3. **Editor file placement.** The query editor writes `QUERY_MESSAGE.md` into a
   conversation directory derived from the storage path, falling back to `root`.

All three assume a local filesystem, preventing JP from running in environments
without one, such as a browser using Web Storage, a database-backed workspace,
or a test harness that avoids temporary directories.

[RFD 073] removes `Workspace::root` during the storage backend refactoring and
threads an explicit `root` parameter to callers that need the project path.
That intermediate step leaves the filesystem assumption intact, since callers
still pass a `Utf8Path` that must point to a real directory.

Replacing that path with a `ProjectFiles` trait object lets callers access
project files through a local directory, an in-memory `HashMap`, or another
backend, while subprocess tools call `materialize()` to obtain a filesystem
view.

### Pain Points

**Tests create temporary directories for project files.** Every integration test
that exercises tool execution or attachment resolution creates a `tempdir` just
so `root` has a valid path.
An in-memory implementation eliminates this overhead and makes tests faster,
more deterministic, and independent of filesystem state.

**Tool execution assumes `current_dir` exists.** `run_tool_command()` calls
`cmd.current_dir(root.as_std_path())`, which fails if the path does not exist.
In-memory workspaces in storage-free tests pass an empty `Utf8PathBuf`, forcing
test helpers to create dummy directories just to satisfy `current_dir`.

**Config loading hardcodes filesystem traversal.** The config pipeline in
`jp_cli` resolves `--cfg` paths by walking three `std::fs` roots: user-global,
workspace root, and user-workspace.
A browser deployment needs a different config resolution strategy, but the
pipeline has no seam for one.

**Attachment handlers receive a raw path.** The `Handler` trait takes `cwd:
&Utf8Path` for path resolution, which works for local files but cannot resolve
resources from non-filesystem sources.

## Design

### The `ProjectFiles` Trait

`ProjectFiles` provides the project file operations that tools, attachments, and
config loading require:

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

Method parameters take `&str` rather than `&Utf8Path` or `&Path` so the trait
stays backend-agnostic.
Every path is relative to the project root: `FsProjectFiles` resolves it against
the root directory, and other backends interpret it as a relative key.
All implementations reject absolute paths and `..` components that escape the
root.

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

These types mirror `jp:host/filesystem` from [RFD 016].
Under the WASM plugin architecture, `jp:host/filesystem` host imports delegate
directly to `ProjectFiles`, sharing the same logical operations across a
different transport.

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

`grep` is a first-class trait method rather than a helper built on `list_dir`
and `read`, which would force every backend to read all files into memory.
Instead, each backend can optimize search directly: `FsProjectFiles::grep` uses
ripgrep-style parallel directory traversal with memory-mapped files, while other
backends can use full-text indexes or database queries.

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
`sync_back: None` that points directly to the real root directory; `drop` is a
no-op with zero allocation or copying.
For `InMemoryProjectFiles`, `materialize()` writes all stored files to a new
temporary directory and sets `sync_back` to a closure that syncs modified files
back into memory on drop.

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

The internal `resolve()` method joins the relative path to `root` and rejects
paths that escape `root` via `..` traversal.

### `NullProjectFiles`

`NullProjectFiles` represents a workspace without a project directory:

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

`NullProjectFiles` replaces `Workspace::new(Utf8PathBuf::new())`, making the
absence of project files explicit instead of passing an empty root path that
fails when used as `current_dir`.

### `InMemoryProjectFiles`

`InMemoryProjectFiles` stores project files in a `HashMap` for tests and
non-filesystem environments:

```rust
#[derive(Debug, Clone, Default)]
pub struct InMemoryProjectFiles {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    label: String,
}
```

`read`, `write`, `exists`, `list_dir`, `delete`, and `rename` operate directly
on the in-memory map, `grep` performs a linear scan over the stored files, and
`materialize()` writes all entries to a temporary directory with a sync-back
closure.
Future non-filesystem backends such as `WebStorageProjectFiles` or a
database-backed store can follow the same structure with a different backing
store.

### `Workspace` Integration

After [RFD 073] removes `Workspace::root`, `Workspace` gains a `ProjectFiles`
field:

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

Callers access the non-optional `project` field through `workspace.project()`,
which returns `&Arc<dyn ProjectFiles>`.

Constructing a filesystem-backed workspace:

```rust
let project = FsProjectFiles::new(&workspace_root);
let backend = FsStorageBackend::new(&storage_path)?
    .with_user_storage(&user_root, name, id)?;

let workspace = Workspace::new(ws_id, Arc::new(project), &backend);
```

Constructing a test workspace without project files:

```rust
let workspace = Workspace::new(
    ws_id,
    Arc::new(NullProjectFiles),
    &InMemoryStorageBackend::default(),
);
```

Constructing a test workspace with in-memory project files:

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

Each caller of `workspace.root()` migrates to `ProjectFiles`:

#### Tool Execution

`run_tool_command()` takes `project: &Arc<dyn ProjectFiles>` instead of `root:
&Utf8Path`, calls `project.materialize()`, and sets `current_dir` to the
materialized path:

```rust
let view = project.materialize()?;
cmd.current_dir(view.path().as_std_path());
```

The tool context JSON changes from `"root": root.as_str()` to `"root":
view.path().as_str()`.
Subprocess tools still receive a filesystem path as both `current_dir` and
`"root"` in the tool context.

#### Attachment Resolution

The `Handler` trait parameter changes from `cwd: &Utf8Path` to `cwd: &Arc<dyn
ProjectFiles>`.
Handlers that resolve local files call `cwd.read(path)`, or `cwd.materialize()`
when they need a filesystem path to spawn a subprocess.
The `file` attachment handler reads file content through `ProjectFiles::read()`
instead of `std::fs::read()`.

#### Editor File Placement

The query editor writes `QUERY_MESSAGE.md` into `.jp/conversations/`, which is
internal storage rather than a project file.
After [RFD 073], the storage backend provides this conversation directory path
instead of `workspace.root()`, so this RFD requires no change.

#### Config Loading

When resolving `--cfg` paths against the user-global, workspace, and
user-workspace roots, `jp_cli`'s config pipeline replaces `workspace.root()`
with `project.display_root()` for logging and `FsProjectFiles`-specific path
access for file resolution.

Full config loading abstraction (such as a `ConfigSource` trait) is deferred
until a non-filesystem deployment defines concrete requirements; the config
pipeline continues to walk the filesystem when resolving relative config paths.

### Relationship to `jp:host/filesystem` (RFD 016)

`ProjectFiles` and the `jp:host/filesystem` WIT interface from [RFD 016] share
the same logical operations (`read`, `write`, `list_dir`, `metadata`).
The WIT interface is the guest-facing contract, and `ProjectFiles` is the
host-side abstraction that backs it on the workspace.
Future VFS-mediated subprocess tools use the same model, exposing `read`,
`write`, and `list_dir` over stdio IPC while the host resolves each request
through `ProjectFiles`.

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

**Dynamic dispatch in the common case.** Production workspaces use a local
filesystem, so `ProjectFiles` adds a vtable lookup and indirection to calls that
could otherwise use `std::fs` directly.
The runtime cost is negligible at I/O boundaries, but the indirection exists
solely for non-filesystem backends.

**`materialize()` is a compatibility escape hatch.** Subprocess tools require
direct filesystem access, forcing non-filesystem backends to create a temporary
directory, write every project file to disk, and sync changes back on drop.
That cost scales with project size and can be significant for large projects.

**`grep` couples the trait to content search.** Most VFS abstractions omit
search because not all backends can implement it efficiently.
However, `grep` is JP's most frequent tool operation, and implementing it atop
`list_dir` and `read` would be too slow on any backend.

**Breaking change to attachment handlers.** Changing `Handler`'s `cwd` parameter
from `&Utf8Path` to `&Arc<dyn ProjectFiles>` requires updating all attachment
handlers (`file_content`, `cmd_output`, `bear_note`, `http_content`,
`mcp_resources`), even though most do not access local files.

## Alternatives

### Keep `root` as `Option<Utf8PathBuf>`

After [RFD 073] removes `root` from `Workspace`, callers could receive
`Option<Utf8PathBuf>` directly without a trait abstraction.

Rejected because it reintroduces the optionality pattern that [RFD 073]
eliminates for storage, forcing every caller to branch on `root.is_some()`.
A polymorphic trait with `NullProjectFiles` matches the storage backend design.

### Existing VFS Crate

Use an existing Rust VFS crate (such as `vfs` or `async-vfs`) that already
provides filesystem and in-memory backends.

Rejected because JP requires first-class `grep`, `materialize()` for subprocess
compatibility, and alignment with the `jp:host/filesystem` WIT interface.
Wrapping and extending a third-party VFS crate to support these operations is
more complex than maintaining a dedicated 10-method trait.

### Combine `ProjectFiles` with Storage Backends

Include `ProjectFiles` in the storage backend traits from [RFD 073] so
`Workspace` holds one fewer trait object and filesystem workspaces share a
single backend struct.

Rejected because project files and workspace storage have distinct access
patterns and lifecycles.
Project files are the user's source tree (read by tools, attached to queries,
and searched with `grep`), whereas storage holds JP's internal state
(conversations, sessions, and locks).
Combining them would force tool-execution test mocks to implement conversation
persistence and vice versa.

## Non-Goals

- **Config loading abstraction.** The config pipeline continues to resolve paths
  against filesystem directories; a `ConfigSource` trait for non-filesystem
  config loading is future work.

- **Cross-project file access.** `ProjectFiles` is scoped to a single project
  root.
  Accessing external paths such as `~/Downloads/` is handled by attachment
  handlers at the CLI layer.

- **Write-back policies.** `MaterializedView` syncs all modified files back on
  drop.
  Selective sync, conflict resolution, and dry-run mode are future work.

- **Production non-filesystem backends.** This RFD defines `FsProjectFiles`,
  `NullProjectFiles`, and `InMemoryProjectFiles`.
  Browser, database, and cloud storage backends are future work.

## Risks and Open Questions

### `materialize()` Scope and Lifetime

For non-filesystem backends, the temporary directory created by `materialize()`
persists on disk until the `MaterializedView` guard drops and syncs changes
back.
Long-running tools hold that disk space for the duration of execution, which is
acceptable because tools typically finish in seconds.

### Partial Materialization

`materialize()` writes the entire project to disk, which can be expensive for
large projects on non-filesystem backends.
Future optimizations could materialize only the files a tool declares or
populate files lazily via FUSE.

### `grep` Scalability on Non-Filesystem Backends

`InMemoryProjectFiles::grep` scans all stored files linearly with a compiled
regex.
That is sufficient for tests, whereas production non-filesystem backends with
large file sets can use full-text search indexes.

### Attachment Handler Migration

Changing `Handler`'s `cwd` parameter from `&Utf8Path` to `&Arc<dyn
ProjectFiles>` touches every attachment crate, though only `file_content` and
`cmd_output` use `cwd` for local file resolution (`bear_note`, `http_content`,
and `mcp_resources` fetch from external sources).

## Implementation Plan

### Phase 1: Define trait and types in a new `jp_project` crate

Define `ProjectFiles`, `DirEntry`, `FileMetadata`, `GrepOpts`, `GrepMatch`,
`MaterializedView`, and the error type in a new `jp_project` crate, kept
separate from `jp_storage` because project files and workspace storage are
distinct concerns.

**Depends on:** Nothing.
**Mergeable:** Yes.

### Phase 2: `FsProjectFiles`

Implement `ProjectFiles` for the local filesystem.
Validate paths to reject absolute paths and `..` traversal, delegate file
operations to `std::fs`, return the real root with no sync-back from
`materialize()`, and implement `grep` with a parallel directory walker and
compiled regex.

**Depends on:** Phase 1.
**Mergeable:** Yes.

### Phase 3: `NullProjectFiles` and `InMemoryProjectFiles`

Implement `NullProjectFiles` and `InMemoryProjectFiles`, with shared compliance
tests across `FsProjectFiles`, `InMemoryProjectFiles`, and `NullProjectFiles`
(verifying error returns on `NullProjectFiles`).

**Depends on:** Phase 1.
**Mergeable:** Yes (parallel with Phase 2).

### Phase 4: Add `ProjectFiles` to `Workspace`

Add `project: Arc<dyn ProjectFiles>` to `Workspace`, expose
`workspace.project()`, and update `Workspace` constructors to accept a
`ProjectFiles` implementation.

**Depends on:** [RFD 073] Phase 4 (Workspace refactor removing `root`), Phase 2,
Phase 3.
**Mergeable:** Yes.

### Phase 5: Migrate tool execution

Update `run_tool_command()` and the tool execution pipeline to take
`ProjectFiles` instead of `root: &Utf8Path`, use `materialize()` for subprocess
`current_dir`, and pass the materialized path in the tool context JSON.

**Depends on:** Phase 4.
**Mergeable:** Yes.

### Phase 6: Migrate attachment handlers

Update the `Handler` trait and all attachment handler implementations to take
`&Arc<dyn ProjectFiles>` instead of `cwd: &Utf8Path`.
Use `ProjectFiles::read()` in `file_content` and `materialize()` in
`cmd_output`.

**Depends on:** Phase 4.
**Mergeable:** Yes (parallel with Phase 5).

### Phase 7: Migrate config pipeline

Replace `workspace.root()` in the config pipeline with `ProjectFiles` access,
sourcing the filesystem root from `FsProjectFiles` while keeping the existing
directory traversal.

**Depends on:** Phase 4.
**Mergeable:** Yes (parallel with Phases 5 and 6).

## References

- [RFD 016]: Wasm Plugin Architecture.
  Defines `jp:host/filesystem` with the same logical operations as
  `ProjectFiles`.
- [RFD 073]: Layered Storage Backend for Workspaces.
  Removes `Workspace::root` and introduces storage backend traits, which this
  RFD complements with a project file abstraction.

[RFD 016]: ../016-wasm-plugin-architecture.md
[RFD 073]: ../073-layered-storage-backend-for-workspaces.md
[RFD D11]: D11-vfs-tool-protocol.md
