# RFD D20: Project, Workspace, and Storage

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-26
- **Requires**: [RFD 079]

## Summary

Today the term "workspace" is overloaded.
It means the project directory, the `.jp/` storage directory, or a Rust struct
that holds both — depending on which file you're reading.
This RFD splits the overloaded term into three named concepts (Project,
Workspace, Storage), formalises a `.jp` marker file that lets workspace storage
live outside the project tree (gitfile-style), and makes config load paths
symmetric across the three config roots.
Schema is unchanged; the changes are conceptual and structural, not data-model.

## Motivation

Three problems in one shape.

**Terminology overloading.** Contributors regularly ask "is `<project>/.jp` the
workspace, or is `<project>` the workspace?"
The doc answer is "the parent", but JP's *observable behaviour* centres on
`.jp/` — that's where state, config, and most editable artifacts live.
Inside the codebase the same ambiguity exists: `Workspace::root()` returns the
project directory while `Storage::root` is `.jp/`, with both calling their value
"root".
The mismatch between user intuition (`.jp/` *is* JP's home) and documented
vocabulary (workspace is the parent) creates friction every time the topic comes
up.

**Storage location ossification.** The library API supports placing storage
elsewhere — `Workspace::find_root(dir, storage_dir)` is parameterised, and
`FsStorageBackend::new(root)` takes any directory.
But `jp_cli` hardcodes `.jp` and treats `<project>/.jp/` as the only valid
storage location.
A read-only project, or any case requiring storage elsewhere, has no user-facing
path to express that.
The capability exists in the library but has been quietly load-bearing only for
tests.

**Config load path asymmetry.** [RFD 035] established multi-root resolution for
`--cfg`.
Its drawbacks section flagged that workspace-flavoured `config_load_paths`
values like `[".jp/config"]` get propagated uniformly to the user-global and
user-workspace roots — producing redundant paths like
`<global>/config/.jp/config/foo.toml` for personal-use files.
RFD 035 parked this as "complexity for marginal benefit."
The benefit has stopped being marginal: contributors place files at the
natural-looking `<global>/config/foo.toml` and get *Missing config file* errors
with a list of search paths none of which match.

These three problems share a root cause.
JP currently lacks a clean distinction between *the project being worked on*,
*the logical JP context for that project*, and *the physical location where that
context's state is stored*.
Once those three are named separately, all three problems become local edits
with bounded scope.

## Design

### Concepts

This RFD introduces three names.
Two are new; one is a redefinition.

| Term                             | Meaning                                                                                                                                                                                                                           |
| -------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Project**                      | The directory the user works in. Where the assistant operates by default. Identified by containing a workspace marker. *(New term in JP vocabulary.)*                                                                             |
| **Workspace**                    | The *logical* JP context attached to a project. Has an ID, a config, conversations, sessions. Implemented as `Workspace` in `jp_workspace`. *(Redefined: previously meant the project directory; now means the logical context.)* |
| **Workspace marker**             | A `.jp` entry at the project root. Either a directory (storage colocated with the project) or a file (storage elsewhere — gitfile-style pointer). The marker is what makes a directory a project. *(New term in JP vocabulary.)*  |
| **Workspace storage**            | The physical location of the workspace's persistent state. Resolved from the marker. Defaults to `<project>/.jp/`. *(Replaces "storage root" in user-facing language.)*                                                           |
| **User-workspace storage**       | Per-user, per-workspace state at `$XDG_DATA_HOME/jp/workspace/<name>-<id>/`. Holds sessions, locks, and optionally user-private conversations. Linked to workspace storage by ID. *(Existing concept, name preserved.)*           |
| **User-global config**           | At `$XDG_CONFIG_HOME/jp/config.toml`. Personal to the user, applies across all workspaces. *(Existing concept.)*                                                                                                                  |
| **Override config** (`.jp.toml`) | Zero or more files at any sub-path inside the project, walked from cwd up to the project root, applied as additional config layers on top of the workspace primary config. *(Existing feature, semantics preserved.)*             |

The terms removed or repurposed:

- "Workspace root" (when meaning the project directory) → **project root**.
- "Storage root" (when meaning `.jp/`) → **workspace storage**.
- `Workspace::root()` → split into `Workspace::project_root()` and
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

The marker file's content is a single line specifying the absolute or
`~`-expanded path to the workspace's storage directory.
Multi-line or structured formats are explicitly out of scope for this RFD; the
simplest thing that works is one line of path.
Format extensions can come later if they earn their keep.

`Workspace::find` walks up from `cwd` looking for `.jp` (file or directory,
whichever appears first).
On match:

- `.jp/` directory → `storage_root = <project>/.jp/`, `project_root =
  <parent>`.
- `.jp` file → read it, resolve the path inside, `storage_root = <that path>`,
  `project_root = <parent>`.

### File system layout

Canonical (colocated) layout:

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

The symlink rename (`storage` → `workspace_storage`) avoids ambiguity now that
"storage" is a load-bearing term in the surrounding vocabulary.

### API surface

`Workspace` gets explicit accessors and stops overloading "root":

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

`Workspace::find` replaces today's `find_root`.
It does the marker resolution end-to-end:

1. Walk up from `cwd` looking for a `.jp` entry (file or directory).
2. Found a directory → `storage_root = <project>/.jp/`.
3. Found a file → read it, parse one line as a path, resolve relative to the
   file's directory if needed, `storage_root = <that path>`.
4. Construct the `Workspace` with `project_root = <parent of marker>` and
   `storage_root = <resolved>`.

The library stays parameterised over the marker name (today `.jp`, configurable
in tests) so swapping conventions in tests stays cheap.

`FsStorageBackend::new(storage_root)` is unchanged — it already takes the
storage location and doesn't need to know about the project.
The split between `Workspace` (logical context, knows project + storage) and
`FsStorageBackend` (knows only storage) maps cleanly to the new vocabulary.

### CLI surface

`--workspace <path>` becomes lenient: it accepts any of:

- A project root (`/path/to/project`) — `find_root` ascent locates the marker.
- A workspace marker (directory or file) — used directly.
- A workspace storage path (when storage is elsewhere) — used directly.

All three resolve to the same workspace.
The user thinks "I'm pointing at this workspace" without caring which physical
thing they typed.

`jp init` creates a colocated workspace at `<cwd>/.jp/` (directory).
Default behaviour is unchanged.

`jp init --storage <path>` (future, not blocking this RFD) creates `<path>/` as
the storage and `<cwd>/.jp` as a marker file pointing to it.

`jp config edit` (the well-developed version, follow-up RFD) supports the common
editing flow without requiring users to know where storage lives — but
`<storage>/config.toml` remains the answer if they go looking, which preserves
the project's "raw editable, not hidden" rule.

### Config load paths

All three roots become structurally symmetric.
Each root has a *config sandbox*:

| Root           | Resolution base               |
| -------------- | ----------------------------- |
| User-global    | `<global>/config/`            |
| Workspace      | `<workspace storage>/config/` |
| User-workspace | `<user-workspace>/config/`    |

`config_load_paths` is interpreted relative to each root's sandbox.
The default for all three roots is `[""]` — search the sandbox itself.

Source-scoping (the contribution from the abandoned D20 draft) is preserved:
each config source's `config_load_paths` configures only its own root.
A workspace `.jp/config.toml` setting `config_load_paths = ["my-extras"]` adds
that path to the *workspace's* search; it does not propagate to the user-global
or user-workspace roots.

Defaults use `MergeableVec` with `discard_when_merged: true`, matching the
existing `default_attachments` pattern in
`crates/jp_config/src/conversation.rs`.
As soon as a user sets any `config_load_paths` value in a given root's config,
the default is discarded and the user's list is the list.
"What I wrote is what I get."

#### Resolution example

Given:

```
<workspace storage>/config/foo.toml
<global>/config/foo.toml
<user-workspace storage>/config/foo.toml
```

…and no user-set `config_load_paths` anywhere, `jp q -c foo` resolves to:

1. `<global>/config/foo.toml` (user-global, lowest precedence)
2. `<workspace storage>/config/foo.toml` (workspace)
3. `<user-workspace storage>/config/foo.toml` (user-workspace, highest)

All three are loaded and merged in that order.
Same cross-root semantics as [RFD 035]; cleaner physical layout because configs
follow workspace storage wherever storage goes (gitfile-style, default, or
user-workspace).

#### Files outside the sandbox

The symmetric model puts a constraint on `--cfg <name>` lookups: named configs
must live inside the relevant root's `config/` sandbox.
Files elsewhere remain reachable via:

- `--cfg <path>` with an explicit path (e.g. `jp q -c ./agents/foo.toml`) —
  bypasses load-path resolution entirely.
- `extends = ["../agents/foo.toml"]` from any config file — `extends` paths
  resolve relative to the file containing them via `to_logical_path`, which
  permits `..` and arbitrary navigation.

The trade-off accepted: arbitrary-project-path *named-shortcut* discoverability
is lost.
Direct paths and explicit extends still work for any reachable file.

#### CLI and per-conversation `config_load_paths`

`config_load_paths` set via `--cfg config_load_paths=...` or in a
per-conversation config layer remains stored-but-unused, as documented in [RFD
079].
There is no natural root for those layers, and the pipeline has already finished
discovering files by the time those layers are applied.

### Error message

The "Searched in:" list in `MissingConfigFile` errors gains structure under
per-root resolution.
Today the list is flat; it should be grouped by root, showing the effective load
paths for each:

```
searched
  user-global    [/Users/jean/Library/Application Support/jp/config]
    - (root)
  workspace      [/Users/jean/Projects/.../my-feature/.jp]
    - (root)
  user-workspace [/Users/jean/Library/Application Support/jp/workspace/my-feature-otvo8/config]
    - (root)
```

The exact rendering is implementation detail; the underlying data must carry the
root association.

## Drawbacks

- **Breaking change.** Every reference to `workspace.root()` in `jp_cli` needs
  review.
  Tests that set `config_load_paths` to workspace-flavoured values need updates.
  Any existing user setup with files at `<global>/config/.jp/config/<name>.toml`
  will stop resolving via the default; users either move files to
  `<global>/config/<name>.toml` or set `config_load_paths = [".jp/config"]`
  explicitly in their global config.
  Pre-release status accepts this; once released this would be a much larger
  commitment.

- **`Workspace::find_root` removal.** Existing callers that use this name break.
  Bounded — most callers want the project root and can migrate trivially to
  `project_root()`.

- **Implicit source-scoping.** The rule that "this file's `config_load_paths`
  configures *this* root" is invisible from inside a single config file.
  A contributor reading `<global>/config.toml` can't tell from the file alone
  that its `config_load_paths` only affects user-global resolution.
  This is documentation-mitigated; `jp config explain` (future, [RFD 060]) is
  the more permanent answer.

- **Symlink rename in user-workspace dir.** Existing user-workspace storage
  directories have a `storage` symlink.
  Migrating them to `workspace_storage` requires a one-time fixup (rename,
  recreate).
  Pre- release means we can do this at startup with no user-visible cost.

## Alternatives

- **Status quo + better docs.** Document the project/`.jp/` distinction more
  carefully, leave the code structure unchanged.
  Cheapest.
  Doesn't address the API-side overloading or the storage-elsewhere capability
  gap; doesn't resolve [RFD 035]'s parked drawback.

- **Workspace = `.jp/` (Framing B).** Redefine `Workspace` to mean the storage
  directory directly.
  Maps to the git mental model (`.git/` ≈ `.jp/`).
  All three config roots become uniform without further ceremony.
  Forecloses storage-elsewhere because workspace and storage collapse into one
  path.
  Rejected because storage-elsewhere is a stated long-term requirement
  (read-only projects, programmatic access to workspaces independent of project
  location).

- **Drop "workspace" entirely.** Use only "project" and "storage".
  Sidesteps the term overloading.
  Loses a useful concept (the logical pairing) and doesn't help when discussing
  things that span project and storage — particularly the `Workspace` struct in
  code, which would have to be renamed to something like `JpContext` for no
  obvious gain.

- **Coined term for `.jp/`.** Keep "workspace" = project, invent a new word for
  `.jp/` (vault, den, etc.).
  Rejected because invented vocabulary rarely sticks when a perfectly good word
  ("storage") describes the role.

- **Per-root scoped `config_load_paths` schema.** Make
  `config_load_paths.workspace`, `.user_global`, `.user_workspace` first- class
  schema fields.
  Mechanically equivalent to source-scoping.
  Rejected because source-scoping reads the same information from where the file
  *lives* without changing the schema, and the schema change would break JSON
  Schema consumers.

## Non-Goals

- **Schema change.** `config_load_paths` stays a flat `Vec<RelativePathBuf>` in
  the TOML/JSON surface.
  Resolution rule changes; schema does not.

- **Workspace-less behaviour.** `jp query` outside any workspace remains an
  error today.
  Future work (global default workspace, ephemeral mode, on-demand workspace
  creation, etc.) is out of scope here.
  The design supports all of those by leaving `Workspace::find` returning `None`
  when no marker is found and letting the CLI handle that case in its own RFD.

- **`jp config edit/set/get` UX surface.** The well-developed config-editing
  command is the right answer to "users shouldn't need to know where storage
  lives", but its sub-command surface is its own RFD.

- **Storage-elsewhere CLI ergonomics.** This RFD defines the marker file
  mechanism.
  The `jp init --storage <path>` command, the marker file format extensions, and
  any `jp storage move` / `jp storage check` operations are follow-ups.

- **Conversation storage policy.** Where conversations physically land
  (workspace storage vs user-workspace storage) is governed by existing RFDs;
  this RFD doesn't change persistence routing.

- **Within-root `config_load_paths` semantics.** First-match-wins within a root
  is preserved unchanged.
  Multiple load paths within a single root remain a priority/organisation
  mechanism, not a layered merge.

- **Renaming `config.d/` or restructuring its role.** The Linux convention earns
  its keep.
  `config.d/` and `config/` remain siblings under the workspace storage with
  their existing roles.

## Risks and Open Questions

- **Marker file format extensions.** "One line, a path" is the minimum and
  enough to ship.
  If we later want to express things like "this storage is read-only", "this
  storage is shared across projects", etc., the format needs to grow.
  We should not invent a structured format proactively, but we should not paint
  ourselves into a corner — the simplest extension path is a TOML format
  `storage_root = "/path"` plus other keys.
  Worth noting in the implementation but not spec'd here.

- **Test churn.** `lib_tests.rs` and `query_tests.rs` set `config_load_paths`
  directly via test helpers and rely on uniform- resolution behaviour.
  They need rewriting.
  Estimated cost: a day of focused work, not a structural blocker.

- **JSON Schema documentation.** `workspace-schema.json` documents
  `config_load_paths`.
  The schema field shape doesn't change, but the description should reflect
  source-scoped resolution.

- **`Workspace` term confusion.** Even after the project/workspace/storage
  split, "workspace" remains overloaded compared to dev-tool conventions (Cargo
  monorepo, Terraform state env, VS Code multi-folder).
  The split resolves the *internal* confusion ("which physical thing is the
  workspace?") but doesn't shed the cross-tool baggage.
  Acceptable risk; the term is generic enough that the redefinition is
  defensible.

- **`.jp.toml` outside a workspace.** The cwd-walk loader currently picks up
  `.jp.toml` files even without a workspace.
  Under this RFD, the intended semantics are: `.jp.toml` is purely an override
  layer that requires a workspace to apply against.
  Walking from a directory that contains `.jp.toml` but has no workspace upward
  should still result in "no workspace", and the `.jp.toml` files should be
  ignored (or at least, warned about).
  Worth confirming during implementation.

## Implementation Plan

**Phase 1: Concept and API split.** Introduce `Workspace::project_root()`,
`Workspace::storage_root()`, and `Workspace::find()`.
Migrate call sites in `jp_cli`.
Remove `Workspace::root()` and `find_root()`.
Rename `Storage::root` field to `storage_root` for internal consistency.
No behaviour changes — pure rename and split.

**Phase 2: Marker file mechanism.** Extend `Workspace::find` to handle `.jp` as
a file (in addition to directory).
Define the marker file format ("one line, a path").
Add parsing, error handling, and tests.
No CLI surface yet.

**Phase 3: Config load path symmetry and source-scoping.** Refactor
`load_partial_configs_from_files` to produce source-tagged partials.
Add per-root `config_load_paths` resolution in `config_pipeline.rs`.
Update `MissingConfigFile` error to group by root.
Rewrite affected tests.

**Phase 4: Documentation and migration.** Update
`docs/architecture/ubiquitous-language.md` with Project, Workspace, Workspace
marker, Workspace storage entries.
Update `docs/configuration.md` with the symmetric model and worked examples.
Update [RFD 035]'s drawbacks section with a tip note pointing here.
Add a migration paragraph for early adopters with deep-nested configs.

Phases can land in order; each is independently reviewable.

## References

- [RFD 035] — Multi-root config load path resolution; established cross- root
  merging and parked the redundant-nesting drawback this RFD resolves.
- [RFD 060] — Config explain; the right home for surfacing per-root effective
  load paths and source-scoping decisions to end users.
- [RFD 079] — Config sources and load order; documents the four config sources
  and the stored-but-unused rule for `--cfg`-set `config_load_paths`.
- `crates/jp_workspace/src/lib.rs` — `Workspace::find_root`, the parameterised
  storage-dir walk-up that this RFD generalises.
- `crates/jp_storage/src/lib.rs` — `Storage::root`, the second meaning of
  "root" this RFD disambiguates.
- `crates/jp_config/src/conversation.rs` — `default_attachments`, the precedent
  for `discard_when_merged: true` on a non-empty default that this RFD reuses
  for `config_load_paths`.

[RFD 035]: ../035-multi-root-config-load-path-resolution.md
[RFD 060]: ../060-config-explain.md
[RFD 079]: ../079-config-sources-and-load-order.md
