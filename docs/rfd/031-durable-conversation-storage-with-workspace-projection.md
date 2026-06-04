# RFD 031: Durable Conversation Storage with Workspace Projection

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-05
- **Required by**: [RFD 046]

## Summary

This RFD changes conversation storage so that user-local storage is the source
of truth for all conversations.
Workspace storage (`.jp/conversations/`) becomes a projection — a copy that
exists for git visibility.
Non-local conversations are written to both locations; local conversations are
written only to user-local.
The `local` property becomes a runtime-derived indicator (whether the workspace
copy exists), not a stored flag.

This makes conversations durable across workspace directory deletion - the
primary pain point for git worktree users; without requiring JP to be aware of
git.

## Motivation

JP stores conversations in two locations: workspace storage
(`.jp/conversations/` inside the project directory) and user-local storage
(`~/.local/share/jp/workspace/<name>-<id>/conversations/`).
Today, these are mutually exclusive — a conversation lives in one place or the
other, controlled by the `conversation.user` flag (exposed as `--local`).

This creates a data loss problem for git worktree users.
A typical worktree workflow looks like:

```sh
cd /path/to/project
git worktree add my-feature
cd my-feature
# ... work, use jp query, accumulate conversations ...
cd ..
git worktree remove my-feature   # conversations in .jp/conversations/ are gone
```

Any non-local conversation that was not committed to git is permanently lost.
The user-local storage for that worktree also becomes orphaned, because the
directory name is encoded in the user-local path (`my-feature-<workspace-id>`),
and no future worktree will reconnect to it unless it happens to have the same
directory name.

This is not a theoretical problem — it's a regular occurrence in active
development with multiple worktrees.
The current mitigations (commit conversations to git, or remember to `--local`
every time, or manually move conversations before deleting the worktree) are all
fragile and rely on the user remembering to act before destruction.

The underlying issue is that workspace storage is the *only* copy for non-local
conversations.
If the workspace directory disappears, the data is gone.

## Design

This design builds on the trait-based storage backend from [RFD 073]:
conversation persistence flows through `PersistBackend::write`, loading through
`LoadBackend`, and the filesystem implementation (`FsStorageBackend`, wrapping
the internal `Storage`) is where dual-root reads and writes live.
Backends that do not model two roots (e.g.
`InMemoryStorageBackend`) keep their current single-store behavior.

### Storage Model

Every conversation is always persisted to user-local storage.
This is the durable copy.
For non-local conversations, a second copy is additionally written to the
workspace `.jp/conversations/` directory.
This workspace copy is a projection — it exists so that conversations are
visible to `git status` and can be committed alongside code.

| Conversation type   | User-local       | Workspace      |
| ------------------- | ---------------- | -------------- |
| Non-local (default) | ✓ (durable copy) | ✓ (projection) |
| Local (`--local`)   | ✓ (durable copy) | —              |

When both copies exist and their contents differ, the most recently modified
copy takes precedence (see [Manual Editing and Conflict
Resolution](#manual-editing-and-conflict-resolution)).

User-local storage remains optional in `jp_storage`.
When it is `None` (e.g., a headless server setup, or tests that don't need
durability), the dual-write logic is skipped and JP falls back to single-write
workspace storage — the same behavior as today.
The `--local` flag and `local` derivation are unavailable without user-local
storage; all conversations are workspace-only.

> [!TIP]
> [RFD 039] adds tree-structured parent-child relationships via a `parent_id`
> field in each conversation's metadata, while retaining the flat directory
> layout described here.

### Shared User-Local Storage

Today, user-local storage is keyed by both the worktree directory name and
workspace ID: `~/.local/share/jp/workspace/<name>-<id>/`.
This means each worktree gets its own user-local silo, and removing a worktree
orphans its silo.

This RFD changes the user-local path to be keyed by workspace ID only:

```
~/.local/share/jp/workspace/<workspace-id>/conversations/
```

All worktrees for the same repository share a single user-local store.
This aligns with [RFD 020], which already places session mappings and locks at
`~/.local/share/jp/workspace/<workspace-id>/`.

The `with_user_storage` method on `Storage` drops the `name` parameter.
The rename-on-mismatch logic in `with_user_storage` is replaced by a one-time
migration that moves existing `<name>-<id>` directories to `<id>`.

The same migration imports existing workspace conversations into the shared
user-local store.
Before this RFD, non-local conversations may exist only under
`.jp/conversations/`.
During migration, JP scans the workspace active and archive partitions
(`conversations/` and `conversations/.archive/`) and copies any conversation
missing from user-local into the workspace-ID user-local store, so pre-RFD
workspace-only conversations become durable before dual-write is enabled.
If both roots already hold the same conversation ID, JP resolves the conflict
with the metadata and stream mtime rules from [Manual Editing and Conflict
Resolution](#manual-editing-and-conflict-resolution).
The migration never deletes the workspace copy; it only ensures a durable
user-local copy exists.

### The `local` Property

The `conversation.user` field (serialized as `"local"` in `metadata.json`) is
removed from the `Conversation` struct.
The `local` indicator shown in `jp conversation ls` becomes a runtime-derived
property:

- **`local N`**: The conversation exists in the current workspace's
  `.jp/conversations/`.
  It is projected.
- **`local Y`**: The conversation exists only in user-local storage.
  It is not projected into this workspace.

This derivation is a filesystem check: does a directory matching this
conversation's ID exist in `<workspace>/.jp/conversations/`?
The `local N` / `local Y` states, together with the workspace-only case, are
formalized as `StoragePresence` below.

### Storage Presence and Projection

Two related types capture conversation storage state.
The distinction matters because reads need three states while writes need only
two.

**`StoragePresence`** is the loaded filesystem fact — which roots hold the
conversation.
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

**`Projection`** is the write intent carried by `ConversationLock` /
`ConversationMut` (see [Persistence Behavior](#persistence-behavior)):

```rust
pub enum Projection {
    /// Durable user-local copy only. Selected by `--local`.
    LocalOnly,
    /// Durable user-local copy plus a workspace projection.
    Projected,
}
```

When a lock is acquired, `Projection` is derived from `StoragePresence`:

| `StoragePresence` | Lock `Projection`        |
| ----------------- | ------------------------ |
| `UserLocalOnly`   | `LocalOnly`              |
| `Projected`       | `Projected`              |
| `WorkspaceOnly`   | `Projected` after import |

A `WorkspaceOnly` conversation has no write `Projection` until a write operation
imports it (see [External Conversations](#external-conversations)); read-only
operations never need one.

`StoragePresence` lives in the workspace's in-memory state, keyed by
conversation ID, alongside metadata and events.
It is populated by the cross-root loader: new conversations insert their initial
presence from the creation flags, loading derives presence from root existence,
and import and `jp conversation edit --local` update it.
Because only `FsStorageBackend` knows about two roots, `LoadBackend` exposes
presence; single-store backends (`InMemoryStorageBackend`) report a single-store
default and ignore projection, consistent with the no-user-storage rule below.

Concretely, the index load returns presence alongside each ID rather than a bare
`ConversationId`:

```rust
pub struct ConversationIndexEntry {
    pub id: ConversationId,
    pub presence: StoragePresence,
}
```

`LoadBackend::load_conversation_ids` is replaced or supplemented with a method
returning these entries; the exact name is an implementation detail.

`StoragePresence` and `Projection` are storage-layer types (`jp_storage`):
`Projection` is a parameter to `PersistBackend::write`, and `StoragePresence` is
a load-time fact.
The conversation lock in `jp_workspace` carries the derived `Projection`.

### Persistence Behavior

Whether a conversation is projected into the workspace cannot always be derived
from disk: on first persist, neither directory exists yet, so "write the
workspace copy when one already exists" can never create the initial projection.
The projection intent must therefore be tracked in memory, not inferred from
filesystem state at write time.

`PersistBackend::write` gains a `Projection` argument, using the type defined in
[Storage Presence and Projection](#storage-presence-and-projection).

Persistence is guard-scoped ([RFD 069]): writes happen in
`ConversationMut::flush` and its `Drop` safety net, not from command code.
The `Drop` path has no call site that could thread an argument, so projection
cannot be supplied per-write by the shell.
Instead, **projection is carried by the conversation lock**: `ConversationLock`
/ `ConversationMut` holds a `Projection`, resolved when the lock is acquired,
and passes it to `write` from both `flush` and `Drop`.

Projection is resolved and updated as follows:

- **New conversation**: set from the creation flags — `Projected` by default,
  `LocalOnly` under `--local`.
- **Load / lock acquisition**: derived from the loaded `StoragePresence` via the
  mapping table above.
- **`jp conversation edit --local`**: toggles the carried value (and performs
  the copy/delete in [Toggling Projection](#toggling-projection)).
- **Workspace-only import**: set to `Projected` on the first write (see
  [External Conversations](#external-conversations)).

Given the projection, `write`:

1. Always writes the durable copy to user-local storage.
2. If `Projected`, also writes the workspace copy.

When user-local storage is unavailable (`None`), `FsStorageBackend` ignores
projection and writes only to workspace storage — the single-write behavior of
today.
`LocalOnly` is unreachable in that mode: `--local` is rejected at the CLI
because there is no user-local root to write to.

This replaces the current behavior, where `Storage::persist_conversation` picks
a single root from `metadata.user` and then calls
`remove_stale_conversation_dirs` across *both* roots, deleting any directory for
the same ID that is not the write target.
That cross-root cleanup must be reworked: stale-directory removal runs *per
root*, scoped to the root being written, so a dual-write never deletes the copy
it just wrote in the other root.

The workspace projection is created once (at creation time for projected
conversations, or via `jp conversation edit --local`) and maintained as long as
the workspace directory survives.
If the workspace directory is deleted and recreated, the projection is gone, and
the conversation appears as `local Y` until explicitly re-projected.

### Toggling Projection

`jp conversation edit --local` toggles the workspace projection:

- **`local Y` → `local N`** (project into workspace): Copy the conversation
  from user-local to workspace `.jp/conversations/`.
- **`local N` → `local Y`** (remove from workspace): Delete the workspace copy.
  The user-local copy remains.

### Path and Editor Resolution

Several commands operate on a specific on-disk path: `jp conversation path` and
`jp conversation edit --events` / `--metadata` / `--base-config`.
When both copies exist, these commands need a deterministic rule for which root
to point at.
The rule keeps git-visible editing as the common path:

- **Projected conversation**: prefer the workspace path.
- **Local-only conversation**: use the user-local path.
- **Workspace-only (external) conversation**: use the workspace path until the
  conversation is imported.

For the JP-managed editor commands (`jp conversation edit --events` /
`--metadata` / `--base-config`), JP validates the edit after the editor exits by
loading it back exactly as the next startup would.
A valid edit is synchronized from the edited workspace copy to the durable
user-local copy immediately — byte-for-byte, preserving the user's exact
content — so both copies stay consistent rather than deferring the sync.
An edit that fails to load is never committed: JP prints the error and asks
whether to re-open the editor to fix it or discard the edit (restoring the
original files); a non-interactive run discards it with an error.
Manual edits made outside JP are reconciled lazily on the next load by the
stream/metadata mtime rules below.

### Manual Editing and Conflict Resolution

JP uses plain, pretty-printed JSON files (`events.json`, `metadata.json`)
specifically so that users can edit them by hand.
Tweaking a conversation's context window — removing a noisy tool call, editing
a response, trimming history — is a supported workflow.
The dual-write model must not break this.

The rule is **last-write-wins based on file modification time (mtime)**, applied
to two independently-resolved units:

- **The conversation stream** — `base_config.json` plus `events.json` together.
  A conversation's stream is loaded from these two files as a unit (via
  `ConversationStream::to_parts`), so they must be selected from the *same*
  root.
  `base_config.json` is written once at creation but is independently
  user-editable (`jp conversation edit --base-config`), so a root's stream mtime
  is `max(mtime(base_config.json), mtime(events.json))`.
  JP loads the stream (both files) from whichever root has the newer stream
  mtime, and never pairs an `events.json` from one root with a
  `base_config.json` from the other.
  For a legacy root that predates the split (base config packed into
  `events.json`, no `base_config.json` file), the stream mtime is
  `mtime(events.json)`; JP writes the current three-file layout on the next
  persist.
- **The metadata** — `metadata.json`, resolved on its own mtime.

Whichever copy is newer in each unit is the version JP loads into memory.
If the two mtimes are equal, user-local wins: with no evidence the workspace
projection is newer, the durable copy stays authoritative.

On persist, JP writes to both locations, bringing their content back into sync.
The write is idempotent: a file is rewritten (and its mtime bumped) only when
its serialized bytes differ, so after a successful persist both copies hold
identical content, and unchanged files keep their existing mtimes.

The full load-persist cycle:

1. **Load**: If both copies exist, resolve the stream (`base_config.json` +
   `events.json`) as a unit by the newer `max(mtime(base_config.json),
   mtime(events.json))`, and resolve `metadata.json` independently by its own
   mtime.
2. **Run**: Execute the query, tool calls, etc. The in-memory state reflects the
   user's edits.
3. **Persist**: Write to user-local, then write to workspace (if projected).
   Both copies are now identical.

If only one copy exists (local-only conversation, or workspace copy was deleted
by a worktree removal), there is nothing to compare — JP loads what's there.

This means a user can edit either copy between JP runs:

- Edit `.jp/conversations/<id>/events.json` in the workspace (the common case —
  the file is right there in the project directory, visible in the editor's file
  tree).
- Edit `~/.local/share/jp/workspace/<id>/conversations/<id>/events.json` in
  user-local (less common, but works the same way).

In both cases, JP picks up the edit on the next run and propagates it to the
other copy on persist.

### Loading Conversations

When loading the conversation list (e.g., `jp conversation ls`), JP reads from
both user-local and workspace storage:

1. Load all conversation IDs from user-local (authoritative).
2. Load all conversation IDs from workspace `.jp/conversations/`.
3. Merge and **deduplicate by conversation ID**: a projected conversation
   appears in both roots, so the merged set must collapse the two entries into
   one.
   (Today `load_all_conversation_ids` concatenates both roots without
   deduplicating; under dual storage duplicates become the normal case, so the
   scan must dedup by ID.)
   Conversations in user-local are the primary set; conversations that exist
   only in the workspace are included but marked as workspace-only (see
   [External Conversations](#external-conversations)).
4. Derive each conversation's `StoragePresence`, which yields the `local`
   indicator (`Projected` → `local N`, `UserLocalOnly` → `local Y`,
   `WorkspaceOnly` → external).

### External Conversations

Conversations can appear in the workspace that do not exist in user-local.
This happens when another contributor commits a conversation to git and you pull
it.

These conversations have `StoragePresence::WorkspaceOnly`.
They are shown in `jp conversation ls` with a distinct indicator marking the
absence of a user-local copy.

A workspace-only conversation is **imported** into user-local (copied workspace
→ user-local) on the first operation that mutates it for continued use, after
which it follows the normal dual-write rules:

- `jp query` (assistant turns, tool calls).
- `jp conversation edit` (`--events`, `--metadata`, `--base-config`, or a
  property edit).
- `jp conversation archive` / `unarchive` (so archive state is recorded in the
  durable copy too).
- Title generation (see [RFD 053]), which persists the conversation.
- Any config mutation that records a delta to the stream.

`jp conversation rm` is the exception: it deletes every copy that exists (both
roots) without importing first.

Read-only operations — `jp conversation show`, `print`, `ls`, and `path` on a
workspace-only conversation — read directly from the workspace without
importing.

### Archiving

[RFD 071] gives conversations active and archive partitions
(`conversations/.archive/`).
Archiving must respect projection:

- **Projected conversation**: archive in *both* roots, so the workspace
  projection and the durable copy move into their respective archive partitions
  together.
  (Today `Storage::archive_conversation` returns after archiving the first root
  it finds — under dual storage that would leave the other copy active.
  The implementation must archive every root in which the conversation exists.)
- **Local-only conversation**: archive in user-local only.
- **Workspace-only (external) conversation**: imported first (see [External
  Conversations](#external-conversations)), then archived in both roots.

`jp conversation ls --archived` scans the archive partition of both roots and
**deduplicates by ID**, exactly as the active listing does.

Unarchive is the inverse: a conversation archived from both roots is restored to
the active partition of both roots; a conversation archived from user-local only
is restored there only.
Restoring re-establishes the projection state the conversation had when it was
archived.

mtime conflict resolution does **not** apply to archived conversations — they
are not the target of live edits, so JP restores whatever copy each root holds
rather than comparing mtimes across roots.

## Drawbacks

**Double disk I/O for non-local conversations.** Every persist operation writes
to two locations.
For JSON conversation files this is negligible, but it is strictly more work
than the current single-write model.

**Divergence risk.** If JP crashes between writing to user-local and workspace
(or vice versa), the two copies can diverge.
The mtime-based resolution (see [Manual Editing and Conflict
Resolution](#manual-editing-and-conflict-resolution)) handles this gracefully —
whichever copy was written last is used on next load, and the next successful
persist re-syncs both copies.
However, if the crash happens after writing user-local but before writing
workspace, the workspace copy may be stale until the next JP run.
Users who inspect workspace files directly between JP runs could see outdated
content.

**Conversation accumulation in user-local.** Because user-local is the durable
store, conversations accumulate there indefinitely (across worktree lifetimes).
The existing ephemeral conversation cleanup (`expires_at`) helps, but users who
create many conversations across many worktrees may eventually want a dedicated
cleanup command or policy.
This is not new — user-local conversations already accumulate today — but the
volume increases when all conversations go through user-local.

## Alternatives

### Keep current model, add evacuation command

A `jp conversation evacuate` command that moves all workspace conversations to
user-local before worktree removal.
This requires the user to remember to run it (or configure a git hook).

Rejected because it doesn't solve the problem — it shifts it from "remember to
use `--local`" to "remember to run evacuate."
Forgetting either one results in data loss.

### Store conversations at the bare repo level

For worktree setups, store conversations in a shared location above the
worktrees (e.g., `/path/to/project/.jp/conversations/`).
All worktrees read from and write to this shared directory.

Rejected because it requires JP to understand git worktree topology (resolving
the common git dir), which violates the goal of keeping JP git-unaware.
It also introduces write contention between concurrent worktrees.

### Sync conversations across workspaces

Keep per-worktree storage but replicate conversations bidirectionally across all
workspaces with the same ID.

Rejected because bidirectional sync is the wrong model.
Conversations are contextual to a branch or task — syncing a feature-a
conversation into the feature-b worktree is undesirable.
The problem is durability, not distribution.

### `.jp` as a file (like `.git` in worktrees)

Make `.jp` a pointer file that redirects to shared storage, similar to how git
worktrees use a `.git` file pointing to the common git dir.

Rejected because `.jp/conversations/` needs to contain real files for `git
status` visibility.
A pointer file would make conversations invisible to git, defeating one of the
two core requirements.

### Default all conversations to `--local`

Make `Conversation::default()` set `user: true`, so all conversations go to
user-local by default.
Users who want git-visible conversations opt in explicitly.

Rejected because it inverts the current expectation that conversations are
visible in the workspace by default.
The dual-write approach preserves the default behavior (workspace-visible) while
adding durability.

## Non-Goals

- **Git awareness.** JP does not detect whether it is running inside a git
  worktree, does not resolve the common git directory, and does not read or
  write git-specific metadata.
  The design works for any workflow where the workspace directory might be
  deleted — worktrees, temporary clones, or manual cleanup.

- **Three-way merge conflict resolution.** If both copies are edited between JP
  runs (e.g., a user edits the workspace copy while a background process edits
  the user-local copy), the older edit is silently overwritten by the newer one.
  JP does not attempt to merge concurrent edits.
  In practice, users edit one copy (almost always the workspace one), so this is
  unlikely to cause issues.

- **Cross-machine sync.** User-local storage is local to the machine.
  This RFD does not address synchronizing conversations across machines.

- **Conversation garbage collection policy.** This RFD does not define when or
  how old conversations in user-local should be cleaned up.
  The existing `expires_at` mechanism continues to work.
  A dedicated cleanup UX is future work.

## Risks and Open Questions

### Migration of existing user-local directories

Renaming `<name>-<id>` directories to `<id>` needs to handle the case where
multiple worktrees have already created separate user-local directories (e.g.,
`main-otvo8` and `feature-a-otvo8`).
The migration must merge their contents without losing conversations.
If two directories contain a conversation with the same ID (unlikely but
possible if both worktrees were used concurrently), the most recently modified
copy should win.
The workspace-to-user-local import is non-destructive — it only copies
conversations missing from user-local and never deletes the workspace copy, so
no data is ever lost.
The eager import runs once, gated on the user-local `<id>` directory not yet
existing.
If it fails partway after creating that directory, a later startup skips the
remaining eager import: the conversations it missed keep
`StoragePresence::WorkspaceOnly` and are imported lazily on their first write —
the same path conversations committed by other contributors take after
migration.

### Workspace copy freshness

The workspace copy is updated on every persist, but only if the workspace copy
already exists.
If a user manually deletes a conversation from `.jp/conversations/` (e.g., via
`git checkout` or `git clean`), JP will not recreate it — the conversation
silently becomes local-only.
This is arguably correct behavior (the user deleted it), but might surprise
users who expect workspace conversations to be self-healing.

### Performance of `local` derivation

Deriving the `local` indicator requires checking the filesystem for each
conversation when listing.
For workspaces with many conversations, this could add latency to `jp
conversation ls`.
In practice, the number of conversations per workspace is small (tens, not
thousands), so this is unlikely to be a problem.
If it becomes one, the check can be batched into a single directory listing.

### Sanitizer behavior across roots

`LoadBackend::sanitize` (the filesystem implementation) scans both roots and
trashes invalid conversation directories independently.
Under dual storage a conversation can have a corrupt copy in one root and a
valid copy in the other.
The independent scan is acceptable: the corrupt copy is trashed and the valid
copy survives, after which the stream/metadata mtime resolution loads the
surviving copy.
This RFD does not change sanitizer scoping; it relies on the scan staying
per-root.

## Implementation Plan

The phases are ordered so storage is never left in a state where the loader sees
data it cannot represent.
Cross-root loading lands *before* dual-write, because once two roots hold the
same conversation the existing single-root loader would surface duplicate IDs.

### Phase 1: Shared User-Local Storage

Change the user-local storage path from `<name>-<id>` to `<id>`.
Add migration logic to merge existing per-worktree user-local directories.
Import existing workspace conversations (active and archive partitions) into
user-local storage so pre-RFD workspace-only conversations become durable before
dual-write is enabled.
Update `with_user_storage` to drop the `name` parameter.

Depends on: nothing.

### Phase 2: Cross-Root Load Model

Teach `FsStorageBackend` / `LoadBackend` to load from both roots: deduplicate
IDs by conversation ID, compute `StoragePresence` per conversation
(`UserLocalOnly` / `Projected` / `WorkspaceOnly`), and resolve conflicts —
`metadata.json` by its own mtime, the stream (`base_config.json` +
`events.json`) as a unit by the newer `max(mtime(base_config.json),
mtime(events.json))`.
Expose `StoragePresence` through `LoadBackend` and store it in the workspace's
in-memory state so the lock can derive `Projection` from it.

This must precede dual-write so the loader can represent a conversation that
exists in both roots.

Depends on: Phase 1.

### Phase 3: Dual-Write Persistence

Implement the dual-write through the [RFD 073] backend traits, using the
projection model from Phase 2:

- Add the `Projection` argument to `PersistBackend::write` (and update the
  `InMemoryStorageBackend` / `NullPersistBackend` implementations).
- Carry `Projection` on `ConversationLock` / `ConversationMut`, resolved at lock
  acquisition, so `flush` and `Drop` persist with the correct projection.
- In `FsStorageBackend` / `Storage::persist_conversation`, always write the
  user-local copy and, when `Projected`, the workspace copy.
- Rework `remove_stale_conversation_dirs` so stale-directory cleanup is scoped
  to the root being written, so a dual-write does not delete the copy just
  written in the other root.
- Remove the stored `conversation.user` field.
  Store the derived projection state outside `Conversation` metadata (carried by
  the lock), and derive the `local` indicator at load time from workspace-copy
  existence.

Depends on: Phase 2.

### Phase 4: External Conversation Import

Add import logic: when JP performs a write operation on a workspace-only
conversation, copy it to user-local first, then follow the normal dual-write
rules.
Update `jp conversation ls` to display workspace-only conversations with
appropriate indicators.

Depends on: Phase 2 and Phase 3.

### Phase 5: Toggle Projection (`jp conversation edit --local`)

Change `--local` toggling to copy-to-workspace / delete-from-workspace instead
of the current move-between-storage-locations behavior, updating the carried
projection state.

Depends on: Phase 3.

### Phase 6: Archive, Unarchive, Remove, Path, and Editor Behavior

Update `archive_conversation` / `unarchive_conversation` to act on every root in
which the conversation exists (not just the first found), deduplicate `ls
--archived`, and apply the path-preference rule to `jp conversation path` and
`jp conversation edit` (including the immediate re-sync after a managed editor
command).
Confirm `remove_conversation` continues to delete all copies in both roots.

Depends on: Phase 2 and Phase 3.

### Phase 7: Tests

Add filesystem-specific tests for dual storage: projection on first persist,
dual-write without cross-root clobber, stream-unit conflict resolution
(including a `base_config.json`-only edit), ID dedup, import on first write, and
archive/unarchive across roots.
The backend parity suite is insufficient on its own — `InMemoryStorageBackend`
does not model two roots, so projection behavior needs dedicated
`FsStorageBackend` tests.

Depends on: the phases under test.

### Phase 8: Glossary

Update `docs/architecture/ubiquitous-language.md` with a "Workspace Projection"
entry when this RFD is implemented.

## References

- [RFD 020: Parallel Conversations][RFD 020] — removes
  `active_conversation_id`, introduces per-session conversation tracking and
  conversation locks.
  This RFD's shared user-local storage aligns with RFD 020's session and lock
  storage paths.

- [RFD 073: Layered Storage Backend for Workspaces][RFD 073] — introduces the
  `PersistBackend` / `LoadBackend` / `FsStorageBackend` traits this design
  builds on.

- [RFD 071: Conversation Archiving][RFD 071] — adds the active/archive
  partitions whose dual-root behavior this RFD specifies.

[RFD 020]: 020-parallel-conversations.md
[RFD 039]: 039-conversation-trees.md
[RFD 046]: 046-nested-workspace-projection.md
[RFD 053]: 053-auto-refresh-conversation-titles.md
[RFD 069]: 069-guard-scoped-persistence-for-conversations.md
[RFD 071]: 071-conversation-archiving.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
