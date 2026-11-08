# RFD D20: Project, Workspace, and Storage

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-26
- **Requires**: [RFD 079]
- **Summary**: Separates Project, Workspace, and Storage concepts, supports
  external `.jp` marker files, and unifies `config_load_paths` across roots.

## Summary

This RFD separates the overloaded term "workspace" into three concepts:
**Project** (the user's working tree), **Workspace** (the logical JP context),
and **Storage** (the on-disk state directory).
It also introduces a gitfile-style `.jp` marker file so storage can live outside
the project tree, and makes `config_load_paths` resolution symmetric across the
user-global, workspace, and user-workspace roots.

## Motivation

**Terminology overloading.** Contributors regularly ask whether the workspace is
`<project>` or `<project>/.jp`.
Documentation defines the workspace as the parent directory, yet JP stores its
state, configuration, and editable artifacts inside `.jp/`.
The codebase has the same ambiguity: `Workspace::root()` returns the project
directory while `Storage::root` points to `.jp/`, with both named `root`.

**Storage location ossification.** The library API already supports external
storage: `Workspace::find_root(dir, storage_dir)` is parameterized, and
`FsStorageBackend::new(root)` accepts any directory.
However, `jp_cli` hardcodes `.jp` and treats `<project>/.jp/` as the only valid
storage location, so only tests can place storage outside the project tree.
Users working in a read-only project have no way to store workspace state
elsewhere.

**Config load path asymmetry.** [RFD 035] introduced multi-root resolution for
`--cfg`, propagating workspace-oriented `config_load_paths` values such as
`[".jp/config"]` to the user-global and user-workspace roots.
That propagation produces redundant paths like
`<global>/config/.jp/config/foo.toml` for personal config files.
[RFD 035] deferred fixing this as "complexity for marginal benefit", but in
practice contributors place files at `<global>/config/foo.toml` and hit *Missing
config file* errors because none of the searched paths match.

All three problems stem from conflating the project directory, the logical JP
context for that project, and the directory where that context's state is
stored.
Separating those three concepts makes each fix a small, local change.

## Design

### Concepts

The table below defines the terms used across this design and how they relate to
existing vocabulary:

| Term                             | Meaning                                                                                                                                                                                                                 |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Project**                      | The directory the user works in and where the assistant operates by default, identified by a workspace marker. *(New term.)*                                                                                            |
| **Workspace**                    | The logical JP context attached to a project, holding an ID, configuration, conversations, and sessions. Implemented as `Workspace` in `jp_workspace`. *(Redefined from the project directory to the logical context.)* |
| **Workspace marker**             | A `.jp` entry at the project root that marks the directory as a project: either a directory (colocated storage) or a gitfile-style file pointing to storage elsewhere. *(New term.)*                                    |
| **Workspace storage**            | The directory holding the workspace's persistent state, resolved from the workspace marker. Defaults to `<project>/.jp/`. *(Replaces "storage root" in user-facing terminology.)*                                       |
| **User-workspace storage**       | Per-user, per-workspace state at `$XDG_DATA_HOME/jp/workspace/<name>-<id>/`, linked to workspace storage by ID. Holds sessions, locks, and optional user-private conversations. *(Existing term.)*                      |
| **User-global config**           | Personal configuration at `$XDG_CONFIG_HOME/jp/config.toml` that applies across all workspaces. *(Existing term.)*                                                                                                      |
| **Override config** (`.jp.toml`) | Optional files at any sub-path inside the project, walked from `cwd` up to the project root and layered on top of the primary workspace config. *(Existing term.)*                                                      |

This split replaces or retires three existing names:

- "Workspace root" (meaning the project directory) becomes **project root**.
- "Storage root" (meaning `.jp/`) becomes **workspace storage**.
- `Workspace::root()` splits into `Workspace::project_root()` and
  `Workspace::storage_root()`.

### Workspace marker

A workspace is identified by a `.jp` entry at the project root, in one of two
forms:

```
<project>/.jp/                          ← marker as directory; storage colocated here
```

```
<project>/.jp                           ← marker as file; storage at the path inside
```

A marker file contains a single line with the absolute or `~`-expanded path to
the workspace's storage directory.
Multi-line or structured formats are out of scope for this RFD.

`Workspace::find` walks up from `cwd` until it finds a `.jp` file or directory.
In both cases, `project_root` is the parent directory of `.jp`.
For a `.jp/` directory, `storage_root` is `<project>/.jp/`; for a `.jp` file,
`storage_root` is the path read from the file.

### File system layout

Colocated storage layout (default):

```
<project>/
├── .jp/                       ← workspace marker (directory) + workspace storage
│   ├── .id                    ← workspace ID
│   ├── config.toml            ← workspace primary config
│   ├── config.d/              ← auto-extends drop-ins (default extends pattern)
│   ├── config/                ← --cfg sandbox; everything inside is loadable
│   │   ├── personas/
│   │   │   └── architect.toml ← jp q -c personas/architect
│   │   └── …
│   ├── conversations/
│   ├── mcp/                   ← (existing) MCP tool definitions
│   └── …
├── docs/sub/.jp.toml          ← optional override config at any sub-path
└── …                          ← project files (tools see <project>/ as root
                                 when granted access; assistant has no default
                                 access of its own)
```

Storage-elsewhere layout:

```
<project>/
├── .jp                        ← marker file; contents: path to storage
└── …

<elsewhere>/                   ← workspace storage; same shape as <project>/.jp/
├── .id
├── config.toml
├── config.d/
├── config/
├── conversations/
└── …
```

User-workspace storage (unchanged in shape, naming preserved):

```
$XDG_DATA_HOME/jp/workspace/<name>-<id>/
├── config.toml                ← user-private workspace config
├── config/                    ← user-private --cfg sandbox
├── conversations/             ← user-private conversations
├── sessions/
├── locks/
└── workspace_storage          ← symlink → workspace storage (renamed from `storage`)
```

Renaming the symlink from `storage` to `workspace_storage` clarifies that it
points to the workspace storage directory rather than user-workspace storage.

### API surface

`Workspace` provides explicit accessors instead of overloading `root`:

```rust
impl Workspace {
    /// The project this workspace is attached to.
    pub fn project_root(&self) -> &Utf8Path;

    /// Where this workspace's state physically lives.
    pub fn storage_root(&self) -> &Utf8Path;

    /// Per-user, per-workspace storage location, if user storage is enabled.
    pub fn user_storage_root(&self) -> Option<&Utf8Path>;

    /// Walk up from `cwd` looking for a `.jp` marker (directory or file)
    /// and build the workspace from it.
    pub fn find(cwd: &Utf8Path) -> Result<Option<Self>>;
}
```

`Workspace::find` replaces `find_root` and resolves the marker in one pass:

1. Walk up from `cwd` looking for a `.jp` entry (file or directory).
2. If `.jp` is a directory, set `storage_root` to `<project>/.jp/`.
3. If `.jp` is a file, read its single-line path, resolve it relative to the
   file's parent directory if needed, and set `storage_root` to that path.
4. Construct the `Workspace` with `project_root` set to the marker's parent
   directory and `storage_root` set to the resolved storage path.

The library remains parameterized over the marker name (defaulting to `.jp`) so
tests can override it.

`FsStorageBackend::new(storage_root)` is unchanged because it already takes the
storage path directly and does not need the project root.
`Workspace` holds the logical context (both project and storage paths), while
`FsStorageBackend` manages only storage.

### CLI surface

`--workspace <path>` accepts any of the following and resolves them to the same
workspace:

- A project root (`/path/to/project`), walking up with `find_root` to locate the
  marker.
- A workspace marker (directory or file), used directly.
- A workspace storage path (when storage lives outside the project), used
  directly.

`jp init` continues to create a colocated workspace directory at `<cwd>/.jp/` by
default.
A future `jp init --storage <path>` flag (not blocking this RFD) will create the
storage directory at `<path>/` and write a `<cwd>/.jp` marker file pointing to
it.

A follow-up `jp config edit` command will let users edit configuration without
looking up the storage path, while keeping `<storage>/config.toml` directly
editable on disk.

### Config load paths

All three roots use the same structure, each with its own `config/` sandbox:

| Root           | Resolution base               |
| -------------- | ----------------------------- |
| User-global    | `<global>/config/`            |
| Workspace      | `<workspace storage>/config/` |
| User-workspace | `<user-workspace>/config/`    |

`config_load_paths` is resolved relative to each root's sandbox.
The default for all three roots is `[""]`, which searches the sandbox directory
itself.

`config_load_paths` is source-scoped: each config source configures only its own
root.
Setting `config_load_paths = ["my-extras"]` in a workspace `.jp/config.toml`
adds that path to the workspace search without propagating to the user-global or
user-workspace roots.

Defaults use `MergeableVec` with `discard_when_merged: true`, matching
`default_attachments` in `crates/jp_config/src/conversation.rs`.
Setting `config_load_paths` in a root's config replaces the default list for
that root.

#### Resolution example

Given the following files and the default `config_load_paths` in all roots:

```
<workspace storage>/config/foo.toml
<global>/config/foo.toml
<user-workspace storage>/config/foo.toml
```

Running `jp q -c foo` loads and merges all three files in precedence order:

1. `<global>/config/foo.toml` (user-global, lowest precedence)
2. `<workspace storage>/config/foo.toml` (workspace)
3. `<user-workspace storage>/config/foo.toml` (user-workspace, highest
   precedence)

This preserves the cross-root merge semantics from [RFD 035], while keeping
workspace configs inside workspace storage whether storage is colocated,
external, or in user-workspace storage.

#### Files outside the sandbox

Under the symmetric model, `--cfg <name>` lookups only find named configs inside
each root's `config/` sandbox; arbitrary project paths can no longer be
discovered by short name.
Configs outside the sandbox remain reachable in two ways:

- `--cfg <path>` with an explicit path (such as `jp q -c ./agents/foo.toml`),
  which bypasses `config_load_paths` resolution.
- `extends = ["../agents/foo.toml"]` in any config file, which resolves paths
  relative to the declaring file via `to_logical_path` and permits `..`
  traversal.

#### CLI and per-conversation `config_load_paths`

`config_load_paths` set via `--cfg config_load_paths=...` or in a
per-conversation config layer remains stored but unused, as documented in [RFD
079].
Those layers have no root directory, and file discovery has already completed
before they are applied.

### Error message

`MissingConfigFile` errors group searched paths by root rather than listing them
flat, showing the effective load paths for each root:

```
searched
  user-global    [/Users/jean/Library/Application Support/jp/config]
    - (root)
  workspace      [/Users/jean/Projects/.../my-feature/.jp]
    - (root)
  user-workspace [/Users/jean/Library/Application Support/jp/workspace/my-feature-otvo8/config]
    - (root)
```

The exact formatting can vary, provided the error data associates each searched
path with its root.

## Drawbacks

- **Breaking change.** Every call to `workspace.root()` in `jp_cli` must be
  updated, along with tests that set `config_load_paths` to workspace-relative
  paths.
  Existing setups with files at `<global>/config/.jp/config/<name>.toml` stop
  resolving under the default paths; users must either move those files to
  `<global>/config/<name>.toml` or set `config_load_paths = [".jp/config"]` in
  their global config.
  Making this change before a stable release avoids a harder migration later.

- **`Workspace::find_root` removal.** Callers of `Workspace::find_root` break,
  though the migration is small because most callers want `project_root()`.

- **Implicit source-scoping.** A config file does not show on its own that its
  `config_load_paths` setting applies only to its own root.
  A contributor reading `<global>/config.toml` cannot tell from the file alone
  that `config_load_paths` affects only user-global resolution.
  Documentation covers this for now, and `jp config explain` ([RFD 060]) will
  surface per-root resolution directly.

- **Symlink rename in user-workspace dir.** Existing user-workspace storage
  directories contain a `storage` symlink that must be renamed or recreated as
  `workspace_storage`.
  Because JP is pre-release, startup can migrate this symlink automatically
  without user-visible cost.

## Alternatives

- **Status quo with clearer documentation.** Document the distinction between
  the project directory and `.jp/` while leaving the code unchanged.
  This is the cheapest option, but it leaves `root` overloaded in the API, does
  not support external storage, and leaves the [RFD 035] `config_load_paths`
  drawback unresolved.

- **Workspace = `.jp/` (Framing B).** Redefine `Workspace` to mean the storage
  directory directly, matching the Git mental model (`.git/` and `.jp/`) and
  making all three config roots uniform.
  Rejected because collapsing workspace and storage into one path prevents
  placing storage outside the project tree, which is required for read-only
  projects and project-independent workspace access.

- **Drop "workspace" entirely.** Use only "project" and "storage" to avoid the
  overloaded term.
  Rejected because JP still needs a name for the logical pairing of project and
  storage, and renaming the `Workspace` struct (for example, to `JpContext`)
  adds churn without clarity.

- **Coined term for `.jp/`.** Keep "workspace" for the project directory and
  invent a new word (such as "vault" or "den") for `.jp/`.
  Rejected because "storage" already describes the role without introducing
  custom jargon.

- **Per-root scoped `config_load_paths` schema.** Make
  `config_load_paths.workspace`, `.user_global`, and `.user_workspace`
  first-class schema fields.
  While mechanically equivalent to source-scoping, this duplicates information
  already implied by the file's location and breaks existing JSON Schema
  consumers.

## Non-Goals

- **Schema change.** `config_load_paths` remains a flat `Vec<RelativePathBuf>`
  in TOML and JSON; only its resolution rule changes.

- **Workspace-less behavior.** Running `jp query` outside a workspace remains an
  error.
  Features such as a global default workspace, ephemeral mode, or on-demand
  workspace creation are out of scope; `Workspace::find` returns `None` when no
  marker is found so future CLI changes can handle that case.

- **`jp config edit/set/get` UX surface.** Dedicated config-editing subcommands
  will be specified in a separate RFD.

- **Storage-elsewhere CLI ergonomics.** This RFD defines only the `.jp` marker
  file mechanism.
  Commands such as `jp init --storage <path>`, `jp storage move`, and `jp
  storage check`, along with marker file format extensions, are left to
  follow-up work.

- **Conversation storage policy.** Existing RFDs govern whether conversations
  are stored in workspace storage or user-workspace storage; this RFD does not
  change persistence routing.

- **Within-root `config_load_paths` semantics.** Resolution within a single root
  still uses first-match-wins rather than a layered merge.

- **Renaming `config.d/` or restructuring its role.** `config.d/` and `config/`
  remain sibling directories under workspace storage with their existing roles.

## Risks and Open Questions

- **Marker file format extensions.** A single-line path is enough for initial
  support, but future requirements such as read-only or shared storage may need
  additional metadata.
  If the format needs to grow later, it can accept TOML keys such as
  `storage_root = "/path"` alongside the plain-path format.

- **Test churn.** `lib_tests.rs` and `query_tests.rs` set `config_load_paths`
  through test helpers and rely on uniform cross-root resolution.
  Updating those tests will take focused effort, though it is not a structural
  blocker.

- **JSON Schema documentation.** Although the `config_load_paths` field shape in
  `workspace-schema.json` does not change, its description must be updated to
  explain source-scoped resolution.

- **`Workspace` term confusion.** Even after separating project, workspace, and
  storage, "workspace" carries different meanings in other developer tools
  (Cargo workspaces, Terraform workspaces, VS Code multi-root workspaces).
  The split resolves the ambiguity within JP, which is sufficient.

- **`.jp.toml` outside a workspace.** The `cwd`-walk loader currently picks up
  `.jp.toml` files even when no workspace exists above `cwd`.
  Because `.jp.toml` is an override layer on top of a workspace config, walking
  up from a directory with a `.jp.toml` but no workspace marker should still
  report no workspace and ignore (or warn about) the `.jp.toml` files.

## Implementation Plan

**Phase 1: Concept and API split.** Introduce `Workspace::project_root()`,
`Workspace::storage_root()`, and `Workspace::find()`.
Migrate call sites in `jp_cli`, remove `Workspace::root()` and `find_root()`,
and rename `Storage::root` to `storage_root`.
This phase is a pure rename and split with no behavior changes.

**Phase 2: Marker file mechanism.** Extend `Workspace::find` to read `.jp`
marker files containing a single-line storage path, and add parsing, error
handling, and tests.
This phase adds no new CLI surface.

**Phase 3: Config load path symmetry and source-scoping.** Refactor
`load_partial_configs_from_files` to produce source-tagged partials, resolve
`config_load_paths` per root in `config_pipeline.rs`, group `MissingConfigFile`
search paths by root, and rewrite affected tests.

**Phase 4: Documentation and migration.** Add Project, Workspace, Workspace
marker, and Workspace storage entries to
`docs/wiki/00dxybd-ubiquitous-language.md`.
Update `docs/configuration.md` with the symmetric model, worked examples, and a
migration note for nested global configs, and add a tip note to the drawbacks
section of [RFD 035].

Phases can land in order; each is independently reviewable.

## References

- [RFD 035]: Multi-root config load path resolution, which established
  cross-root merging and deferred the redundant-nesting drawback resolved here.
- [RFD 060]: Config explain, which will surface per-root effective load paths
  and source-scoping decisions to users.
- [RFD 079]: Config sources and load order, which documents the four config
  sources and why `config_load_paths` set via `--cfg` is stored but unused.
- `crates/jp_workspace/src/lib.rs`: `Workspace::find_root`, the parameterized
  storage-directory lookup generalized by `Workspace::find`.
- `crates/jp_storage/src/lib.rs`: `Storage::root`, the storage path field
  disambiguated by `storage_root`.
- `crates/jp_config/src/conversation.rs`: `default_attachments`, the precedent
  for `discard_when_merged: true` on a non-empty default reused for
  `config_load_paths`.

[RFD 035]: ../035-multi-root-config-load-path-resolution.md
[RFD 060]: ../060-config-explain.md
[RFD 079]: ../079-config-sources-and-load-order.md
