# RFD 031: Durable Conversation Storage with Workspace Projection

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-05
- **Tracking Issue**: [\#788]
- **Extended by**: [RFD 106]
- **Summary**: Makes user-local storage the source of truth for conversations
  and treats `.jp/conversations/` as an optional workspace projection.

## Summary

This RFD makes user-local storage the source of truth for all conversations and
treats workspace storage (`.jp/conversations/`) as a projection for git
visibility.
Shared conversations are written to both locations, while user-local
conversations are written only to the user data directory.
This keeps conversations durable when a git worktree or workspace directory is
deleted.

## Motivation

JP stores conversations in two locations: workspace storage
(`.jp/conversations/` inside the project directory) and user-local storage
(`~/.local/share/jp/workspace/<name>-<id>/conversations/`).
Today, these locations are mutually exclusive: the `conversation.user` flag
(exposed as `--local`) places each conversation in one or the other.

This causes data loss in git worktree workflows:

```sh
cd /path/to/project
git worktree add my-feature
cd my-feature
# ... work, use jp query, accumulate conversations ...
cd ..
git worktree remove my-feature   # conversations in .jp/conversations/ are gone
```

Removing the worktree permanently deletes any uncommitted non-local
conversation.
It also orphans the worktree's user-local storage, because the path encodes the
worktree directory name (`my-feature-<workspace-id>`); no future worktree
reconnects to that directory unless it reuses the same name.

Because workspace storage holds the only copy of a non-local conversation,
deleting the workspace directory destroys the conversation.
Current mitigations (committing conversations to git, passing `--local` on every
command, or moving files before removing a worktree) are fragile because they
rely on the user remembering to act before deleting the directory.

## Design

This design builds on the trait-based storage backend from [RFD 073].
Persistence flows through `PersistBackend::write`, loading flows through
`LoadBackend`, and `FsStorageBackend` (wrapping the internal `Storage`)
implements dual-root reads and writes.
Backends that do not model two roots, such as `InMemoryStorageBackend`, keep
their single-store behavior.

### Storage Model

Every conversation is persisted to user-local storage as its durable copy.
Non-local conversations are also written to `.jp/conversations/` in the
workspace.
This workspace copy is a projection that makes conversations visible to `git
status` so they can be committed alongside code.

| Conversation type   | User-local       | Workspace      |
| ------------------- | ---------------- | -------------- |
| Non-local (default) | ✓ (durable copy) | ✓ (projection) |
| Local (`--local`)   | ✓ (durable copy) | -              |

When both copies exist and their contents differ, the most recently modified
copy takes precedence (see [Manual Editing and Conflict
Resolution](#manual-editing-and-conflict-resolution)).

User-local storage remains optional in `jp_storage`.
When it is `None` (for example, in a headless server setup or in tests that do
not need durability), JP skips dual-write and writes only to workspace storage.
Without user-local storage, the `--local` flag and `local` derivation are
unavailable, and all conversations are workspace-only.

> [!TIP]
> [RFD 039] adds tree-structured parent-child relationships via a `parent_id`
> field in each conversation's metadata, while retaining the flat directory
> layout described here.

### Shared User-Local Storage

Today, JP looks up user-local storage by exact directory name
(`~/.local/share/jp/workspace/<name>-<id>/`), giving each worktree a separate
directory that becomes orphaned when the worktree is removed.

This RFD keys user-local storage on the workspace ID while keeping a
human-recognizable prefix:

```
~/.local/share/jp/workspace/<slug>-<id>/conversations/
```

JP locates the user-workspace directory by ID suffix rather than exact name,
scanning `~/.local/share/jp/workspace/` for a directory named `<id>` or ending
in `-<id>`.
Because a workspace ID is a fixed-length `[0-9a-z]` string that never contains
`-`, the suffix match is unambiguous.
All worktrees and clones for the same repository share a single user-workspace
directory regardless of their checkout directory names.
This aligns with [RFD 020], which stores session mappings and locks under the
same per-workspace directory.

`<slug>` names a newly created directory only, so the user can recognize it
among sibling directories.
It defaults to the workspace directory name (a clone into `~/code/my-project`
yields `my-project-<id>`), is not validated, and may be omitted (an absent or
empty slug yields a bare `<id>` directory).
Once the directory exists, it is **never renamed**: a later clone with a
different directory name reuses it as-is.

`Storage::with_user_storage` takes the optional slug alongside the workspace ID.
Instead of renaming directories on a slug mismatch, a one-time migration folds
any sibling user-workspace directories for the workspace into a single
directory.
When multiple legacy `<name>-<id>` directories exist, the directory matching the
current slug wins; otherwise, the most recently modified directory wins.
The winning directory keeps its name, and the remaining directories are merged
into it.

The same migration imports existing workspace conversations into the shared
user-local store.
JP scans the active and archive partitions in the workspace (`conversations/`
and `conversations/.archive/`) and copies any conversation missing from
user-local storage, making existing workspace-only conversations durable before
dual-write begins.
If both roots already hold the same conversation ID, JP resolves the conflict
using the metadata and stream mtime rules in [Manual Editing and Conflict
Resolution](#manual-editing-and-conflict-resolution).
The migration never deletes the workspace copy.

### The `local` Property

The `conversation.user` field (serialized as `"local"` in `metadata.json`) is
removed from the `Conversation` struct.
Instead, `jp conversation ls` derives the `local` indicator at runtime by
checking whether a directory matching the conversation's ID exists in
`<workspace>/.jp/conversations/`:

- **`local N`**: Projected into `.jp/conversations/` in the current workspace.
- **`local Y`**: Present only in user-local storage.

`StoragePresence` formalizes these two states alongside the workspace-only case.

### Storage Presence and Projection

Two types in `jp_storage` represent conversation storage state:
`StoragePresence` for reads (three states) and `Projection` for writes (two
states).

**`StoragePresence`** records which storage roots hold the conversation on disk.
It drives listing, the `local` indicator, path resolution, and the import
decision for external conversations:

```rust
pub enum StoragePresence {
    /// Durable copy only; not projected into this workspace.
    UserLocalOnly,
    /// Durable copy plus a workspace projection.
    Projected,
    /// Present only in the workspace (committed by another contributor, not
    /// yet imported into user-local).
    WorkspaceOnly,
}
```

**`Projection`** represents the write target carried by `ConversationLock` and
`ConversationMut` (see [Persistence Behavior](#persistence-behavior)):

```rust
pub enum Projection {
    /// Durable user-local copy only. Selected by `--local`.
    LocalOnly,
    /// Durable user-local copy plus a workspace projection.
    Projected,
}
```

Acquiring a lock derives `Projection` from `StoragePresence`:

| `StoragePresence` | Lock `Projection`        |
| ----------------- | ------------------------ |
| `UserLocalOnly`   | `LocalOnly`              |
| `Projected`       | `Projected`              |
| `WorkspaceOnly`   | `Projected` after import |

Read-only operations on a `WorkspaceOnly` conversation do not need a
`Projection`.
Write operations import the conversation into user-local storage first, giving
it `Projection::Projected` (see [External
Conversations](#external-conversations)).

The workspace holds `StoragePresence` in memory alongside metadata and events,
keyed by conversation ID.
New conversations set their initial presence from creation flags, the cross-root
loader derives presence from root existence, and import or `jp conversation edit
--local` updates it.
Because only `FsStorageBackend` manages two roots, `LoadBackend` exposes
`StoragePresence` directly; single-store backends (`InMemoryStorageBackend`)
report a single-store default and ignore projection, consistent with the
no-user-storage rule below.

The index load returns presence alongside each ID rather than a bare
`ConversationId`:

```rust
pub struct ConversationIndexEntry {
    pub id: ConversationId,
    pub presence: StoragePresence,
}
```

A method returning `ConversationIndexEntry` replaces or supplements
`LoadBackend::load_conversation_ids`.

### Persistence Behavior

Write-time filesystem state cannot determine whether a conversation should be
projected: on first persist, neither root directory exists yet, so checking
whether a workspace copy already exists would never create the initial
projection.
JP must track projection intent in memory.

`PersistBackend::write` takes a `Projection` argument (see [Storage Presence and
Projection](#storage-presence-and-projection)).
Because persistence is guard-scoped ([RFD 069]), writes happen inside
`ConversationMut::flush` and its `Drop` implementation rather than in command
code.
The `Drop` path cannot accept caller arguments, so `ConversationLock` and
`ConversationMut` hold the `Projection` resolved at lock acquisition and pass it
to `write` from both `flush` and `Drop`.

`Projection` is resolved and updated as follows:

- **New conversation**: set from the creation flags (`Projected` by default,
  `LocalOnly` with `--local`).
- **Load / lock acquisition**: derived from the loaded `StoragePresence` via the
  mapping table above.
- **`jp conversation edit --local`**: toggles the stored `Projection` and copies
  or deletes the workspace directory (see [Toggling
  Projection](#toggling-projection)).
- **Workspace-only import**: set to `Projected` on the first write (see
  [External Conversations](#external-conversations)).

Given a `Projection`, `PersistBackend::write`:

1. Always writes the durable copy to user-local storage.
2. When `Projected`, also writes the workspace copy.

When user-local storage is `None`, `FsStorageBackend` ignores `Projection` and
writes only to workspace storage.
`LocalOnly` is unreachable in this mode because the CLI rejects `--local` when
no user-local root is configured.

Previously, `Storage::persist_conversation` selected a single root from
`metadata.user` and ran `remove_stale_conversation_dirs` across both roots,
deleting any directory for the same ID outside the target root.
Under dual-write, stale-directory cleanup runs per root, scoped to the root
being written, so writing to one root never deletes the copy in the other.

JP creates the workspace projection at conversation creation (or via `jp
conversation edit --local`) and updates it as long as the workspace directory
exists.
If the workspace directory is deleted and recreated, the projection is gone, and
the conversation appears as `local Y` until re-projected.

### Toggling Projection

`jp conversation edit --local` toggles the workspace projection:

- **`local Y` → `local N`** (project into workspace): Copy the conversation
  from user-local to workspace `.jp/conversations/`.
- **`local N` → `local Y`** (remove from workspace): Delete the workspace copy.
  The user-local copy remains.

### Path and Editor Resolution

Commands that operate on an on-disk path (`jp conversation path` and `jp
conversation edit --events` / `--metadata` / `--base-config`) select a root
based on the conversation's presence, preferring the git-visible workspace copy
when available:

- **Projected**: workspace path.
- **Local-only**: user-local path.
- **Workspace-only (external)**: workspace path until imported.

After a managed editor command (`jp conversation edit --events` / `--metadata` /
`--base-config`) exits, JP validates the edit by loading the files back as the
next startup would.
If validation succeeds on a projected conversation, JP immediately copies the
edited workspace files byte-for-byte to user-local storage so both copies stay
consistent.
If validation fails, JP prints the error and prompts whether to reopen the
editor or discard the edit and restore the original files.
In non-interactive mode, the command fails without opening the editor and points
to `jp conversation path`.
Manual edits made outside JP are reconciled on the next load using the mtime
rules below.

### Manual Editing and Conflict Resolution

JP stores conversations as pretty-printed JSON files (`events.json`,
`metadata.json`, `base_config.json`) so users can edit them by hand to remove a
noisy tool call, edit a response, or trim history.
To support hand-editing across two roots, JP resolves conflicts using
**last-write-wins based on file modification time (mtime)** across two
independently resolved units:

- **Conversation stream** (`base_config.json` and `events.json`):
  `ConversationStream::to_parts` loads these two files as a unit, so JP always
  reads both from the same root and never pairs `events.json` from one root with
  `base_config.json` from the other.
  Because `base_config.json` is independently editable via `jp conversation edit
  --base-config`, a root's stream mtime is `max(mtime(base_config.json),
  mtime(events.json))`.
  For a legacy root that packs the base config into `events.json` without a
  separate `base_config.json`, the stream mtime is `mtime(events.json)`, and JP
  writes the three-file layout on the next persist.
- **Metadata** (`metadata.json`): resolved independently by its own mtime.

For each unit, JP loads the copy with the newer mtime.
If the two mtimes are equal, the user-local copy wins so the durable store
remains authoritative.
If only one copy exists (a local-only conversation, or one whose workspace copy
was deleted with a worktree), JP loads that copy directly.

Users can edit either `.jp/conversations/<id>/events.json` in the workspace or
`~/.local/share/jp/workspace/<slug>-<id>/conversations/<id>/events.json` in
user-local storage between runs.
On the next run, the load-persist cycle reconciles the two copies:

1. **Load**: Resolve the stream (`base_config.json` + `events.json`) by the
   newer `max(mtime(base_config.json), mtime(events.json))`, and resolve
   `metadata.json` by its own mtime.
2. **Run**: Execute the query or command against the loaded in-memory state.
3. **Persist**: Write to user-local storage and (if projected) workspace
   storage.
   Writes are idempotent: JP rewrites a file and updates its mtime only when its
   serialized bytes change, leaving both roots with identical content while
   preserving the mtimes of unchanged files.

### Loading Conversations

When loading the conversation list (for example, in `jp conversation ls`), JP
reads from both user-local and workspace storage:

1. Load all conversation IDs from user-local storage (authoritative).
2. Load all conversation IDs from workspace `.jp/conversations/`.
3. Merge and **deduplicate by conversation ID**.
   Because projected conversations appear in both roots,
   `load_all_conversation_ids` must deduplicate by ID rather than concatenating
   both directory listings.
   Conversations in user-local storage form the primary set; conversations
   present only in the workspace are marked as workspace-only (see [External
   Conversations](#external-conversations)).
4. Derive each conversation's `StoragePresence`, which determines the `local`
   indicator (`Projected` → `local N`, `UserLocalOnly` → `local Y`,
   `WorkspaceOnly` → external).

### External Conversations

When a contributor pulls a conversation committed to git by someone else, that
conversation exists in `.jp/conversations/` but not in user-local storage.
It loads with `StoragePresence::WorkspaceOnly`, and `jp conversation ls`
displays a distinct indicator marking the absence of a user-local copy.

JP **imports** a workspace-only conversation (copying it from the workspace to
user-local storage) on the first operation that mutates it for continued use,
after which it follows the standard dual-write rules:

- `jp query` (assistant turns, tool calls).
- `jp conversation edit` (`--events`, `--metadata`, `--base-config`, or a
  property edit).
- `jp conversation archive` / `unarchive` (so archive state is recorded in the
  durable copy too).
- Title generation (see [RFD 053]), which persists the conversation.
- Any config mutation that records a delta to the stream.

Read-only operations (`jp conversation show`, `print`, `ls`, and `path`) read
directly from the workspace without importing.
`jp conversation rm` also skips import and deletes all existing copies across
both roots.

### Archiving

[RFD 071] gives conversations active and archive partitions
(`conversations/.archive/`).
Archiving respects projection:

- **Projected**: archive in both roots so the workspace projection and the
  durable copy move into their respective archive partitions together.
  `Storage::archive_conversation` must archive every root holding the
  conversation rather than returning after the first match.
- **Local-only**: archive in user-local storage only.
- **Workspace-only (external)**: import into user-local storage first (see
  [External Conversations](#external-conversations)), then archive in both
  roots.

`jp conversation ls --archived` scans the archive partition of both roots and
**deduplicates by ID**, matching the active listing.

Unarchiving is the inverse: a conversation archived in both roots is restored to
the active partition of both roots, and a conversation archived only in
user-local storage is restored there only, preserving the projection state it
had when archived.

Archived conversations do not use mtime conflict resolution.
Because they are not the target of live edits, unarchiving restores the copy in
each root without comparing mtimes across roots.

## Drawbacks

**Double disk I/O for non-local conversations.** Every persist writes to two
locations instead of one, though the extra I/O is negligible for small JSON
files.

**Divergence risk on crash.** If JP crashes between writing to user-local
storage and writing to the workspace, the two copies diverge.
On the next run, mtime resolution (see [Manual Editing and Conflict
Resolution](#manual-editing-and-conflict-resolution)) loads whichever copy was
written last, and the next persist resynchronizes both copies.
However, if the crash happens after writing user-local storage but before
updating the workspace, the workspace files remain stale until the next JP run.

**Conversation accumulation in user-local storage.** Because user-local storage
retains every conversation across worktree lifetimes, conversations accumulate
there faster than when only `--local` conversations used it.
Ephemeral conversation cleanup (`expires_at`) still runs, but users who create
many conversations across worktrees may eventually need a dedicated cleanup
command or policy.

## Alternatives

### Add an Evacuation Command

Add a `jp conversation evacuate` command that moves workspace conversations to
user-local storage before worktree removal.

Rejected because it shifts the burden from remembering `--local` to remembering
`evacuate` (or configuring a git hook).
Forgetting either one causes data loss.

### Store Conversations at the Bare Repository Level

Store conversations in a shared location above individual worktrees (for
example, `/path/to/project/.jp/conversations/`) so all worktrees read and write
the same directory.

Rejected because resolving the common git directory requires JP to understand
git worktree topology, violating the goal of keeping JP git-unaware, and sharing
a single store across concurrent worktrees introduces write contention.

### Sync Conversations Across Workspaces

Keep per-worktree storage and replicate conversations bidirectionally across all
workspaces with the same ID.

Rejected because conversations are contextual to a branch or task; syncing a
`feature-a` conversation into the `feature-b` worktree is undesirable.
The problem is durability, not distribution.

### Make `.jp` a Pointer File

Make `.jp` a pointer file that redirects to shared storage, similar to how git
worktrees use a `.git` file pointing to the common git directory.

Rejected because `.jp/conversations/` must contain real files for `git status`
visibility.

### Default All Conversations to `--local`

Set `user: true` in `Conversation::default()` so all conversations go to
user-local storage unless the user opts in to workspace storage.

Rejected because it inverts the expectation that conversations are visible in
the workspace by default.
Dual-write preserves default workspace visibility while adding durability.

## Non-Goals

- **Git awareness.** JP does not detect whether it is running inside a git
  worktree, resolve the common git directory, or read or write git metadata.
  The design works for any workflow where the workspace directory may be
  deleted, including worktrees, temporary clones, and manual cleanup.

- **Three-way merge conflict resolution.** If both copies of a unit are edited
  between JP runs, the newer copy overwrites the older one without merging.

- **Cross-machine sync.** User-local storage is local to a single machine.

- **Conversation garbage collection policy.** Aside from the existing
  `expires_at` mechanism for ephemeral conversations, this RFD does not define
  how old conversations in user-local storage are pruned.

## Risks and Open Questions

### Migrating Existing User-Local Directories

When multiple worktrees have already created separate user-local directories for
the same workspace ID (such as `main-otvo8` and `feature-a-otvo8`), JP selects
one survivor (preferring the slug match if present, otherwise the most recently
modified directory) and merges the others into it without renaming the survivor.
If two directories contain a conversation with the same ID, the most recently
modified copy wins.

Importing workspace conversations into user-local storage only copies missing
conversations and never deletes the workspace copy.
This eager import runs once, gated on the user-local `<id>` directory not yet
existing.
If it fails partway after creating that directory, subsequent startups skip the
remaining eager import: any unimported conversations load with
`StoragePresence::WorkspaceOnly` and are imported lazily on their first write,
following the same path as conversations committed by other contributors.

### Workspace Copy Freshness

JP updates the workspace copy on persist only when the conversation's presence
is `Projected`.
If a user manually deletes a conversation from `.jp/conversations/` (for
example, via `git checkout` or `git clean`), JP does not recreate it; the
conversation becomes local-only (`local Y`) until re-projected with `jp
conversation edit --local`.

### Performance of `local` Derivation

Deriving the `local` indicator checks the filesystem for each conversation when
listing.
Because workspaces typically have tens of conversations rather than thousands,
the latency added to `jp conversation ls` is small, and the check can be batched
into a single directory listing if needed.

### Sanitizer Behavior Across Roots

The filesystem implementation of `LoadBackend::sanitize` scans both roots and
trashes invalid conversation directories independently.
If a conversation has a corrupt copy in one root and a valid copy in the other,
the sanitizer trashes the corrupt copy and leaves the valid copy for
stream/metadata mtime resolution to load.
This relies on sanitization remaining scoped per root.

## Implementation Plan

The phases are ordered so the loader can represent dual-root conversations
before dual-write is enabled; otherwise, once two roots hold the same
conversation, a single-root loader would surface duplicate IDs.

### Phase 1: Shared User-Local Storage

Locate the user-workspace directory by workspace-ID suffix and name a newly
created one `<slug>-<id>` (or bare `<id>` when the slug is absent).
Fold existing per-worktree user-local directories into the chosen directory
(preferring the slug match, then the most recently modified directory) without
renaming the survivor.
Import existing workspace conversations from both the active and archive
partitions into user-local storage so pre-RFD workspace-only conversations
become durable before dual-write is enabled.
Update `with_user_storage` to take the optional slug alongside the ID.

Depends on: nothing.

### Phase 2: Cross-Root Load Model

Update `FsStorageBackend` and `LoadBackend` to load from both roots: deduplicate
by conversation ID, compute `StoragePresence` per conversation (`UserLocalOnly`,
`Projected`, or `WorkspaceOnly`), and resolve conflicts (`metadata.json` by its
own mtime, and the stream `base_config.json` + `events.json` as a unit by the
newer `max(mtime(base_config.json), mtime(events.json))`).
Expose `StoragePresence` through `LoadBackend` and store it in the workspace's
in-memory state so the lock can derive `Projection` from it.

Depends on: Phase 1.

### Phase 3: Dual-Write Persistence

Implement dual-write through the [RFD 073] backend traits using the projection
model from Phase 2:

- Add the `Projection` argument to `PersistBackend::write` (and update
  `InMemoryStorageBackend` and `NullPersistBackend`).
- Carry `Projection` on `ConversationLock` and `ConversationMut`, resolved at
  lock acquisition, so `flush` and `Drop` persist with the target projection.
- In `FsStorageBackend` / `Storage::persist_conversation`, always write the
  user-local copy and, when `Projected`, the workspace copy.
- Scope `remove_stale_conversation_dirs` to the root being written so a
  dual-write does not delete the copy written in the other root.
- Remove the stored `conversation.user` field and derive the `local` indicator
  at load time from workspace-copy existence.

Depends on: Phase 2.

### Phase 4: External Conversation Import

When JP performs a write operation on a workspace-only conversation, copy it to
user-local storage first, then follow the normal dual-write rules.
Update `jp conversation ls` to display workspace-only conversations with a
distinct indicator.

Depends on: Phase 2 and Phase 3.

### Phase 5: Toggle Projection (`jp conversation edit --local`)

Change `--local` toggling to copy to or delete from the workspace directory
instead of moving the conversation between storage roots, and update the lock's
`Projection` state.

Depends on: Phase 3.

### Phase 6: Archive, Unarchive, Remove, Path, and Editor Behavior

Update `archive_conversation` and `unarchive_conversation` to act on every root
holding the conversation rather than stopping after the first match, deduplicate
`ls --archived`, and apply the path-preference rule to `jp conversation path`
and `jp conversation edit` (including the immediate resync after a managed
editor command).
Confirm `remove_conversation` continues to delete all copies in both roots.

Depends on: Phase 2 and Phase 3.

### Phase 7: Tests

Add filesystem tests for dual storage: projection on first persist, dual-write
without cross-root deletion, stream-unit conflict resolution (including a
`base_config.json`-only edit), ID deduplication, import on first write, and
archive/unarchive across roots.
Because `InMemoryStorageBackend` does not model two roots, the backend parity
suite must be supplemented with dedicated `FsStorageBackend` tests.

Depends on: the phases under test.

### Phase 8: Glossary

Update `docs/architecture/ubiquitous-language.md` with a "Workspace Projection"
entry when this RFD is implemented.

## References

- [RFD 020: Parallel Conversations][RFD 020]: removes `active_conversation_id`
  and introduces per-session conversation tracking and conversation locks.
  This RFD's shared user-local storage aligns with RFD 020's session and lock
  storage paths.

- [RFD 073: Layered Storage Backend for Workspaces][RFD 073]: introduces the
  `PersistBackend`, `LoadBackend`, and `FsStorageBackend` traits this design
  builds on.

- [RFD 071: Conversation Archiving][RFD 071]: adds the active and archive
  partitions whose dual-root behavior this RFD specifies.

[RFD 020]: 020-parallel-conversations.md
[RFD 039]: 039-conversation-trees.md
[RFD 053]: 053-auto-refresh-conversation-titles.md
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
[RFD 071]: 071-conversation-archiving.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD 106]: 106-collision-free-conversation-id-allocation.md
[\#788]: https://github.com/dcdpr/jp/issues/788
