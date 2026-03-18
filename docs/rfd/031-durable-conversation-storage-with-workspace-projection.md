# RFD 031: Durable Conversation Storage with Workspace Projection

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-05

## Summary

This RFD changes conversation storage so that user-local storage is the source
of truth for all conversations. Workspace storage (`.jp/conversations/`) becomes
a projection — a copy that exists for git visibility. Non-local conversations
are written to both locations; local conversations are written only to
user-local. The `local` property becomes a runtime-derived indicator (whether
the workspace copy exists), not a stored flag.

This makes conversations durable across workspace directory deletion - the
primary pain point for git worktree users; without requiring JP to be aware of
git.

## Motivation

JP stores conversations in two locations: workspace storage
(`.jp/conversations/` inside the project directory) and user-local storage
(`~/.local/share/jp/workspace/<name>-<id>/conversations/`). Today, these are
mutually exclusive — a conversation lives in one place or the other, controlled
by the `conversation.user` flag (exposed as `--local`).

This creates a data loss problem for git worktree users. A typical worktree
workflow looks like:

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
development with multiple worktrees. The current mitigations (commit
conversations to git, or remember to `--local` every time, or manually move
conversations before deleting the worktree) are all fragile and rely on the user
remembering to act before destruction.

The underlying issue is that workspace storage is the *only* copy for non-local
conversations. If the workspace directory disappears, the data is gone.

## Design

### Storage Model

Every conversation is always persisted to user-local storage. This is the
durable copy. For non-local conversations, a second copy is additionally written
to the workspace `.jp/conversations/` directory. This workspace copy is a
projection — it exists so that conversations are visible to `git status` and can
be committed alongside code.

| Conversation type    | User-local       | Workspace      |
|----------------------|------------------|----------------|
| Non-local (default)  | ✓ (durable copy) | ✓ (projection) |
| Local (`--local`)    | ✓ (durable copy) | —              |

When both copies exist and their contents differ, the most recently modified
copy takes precedence (see [Manual Editing and Conflict
Resolution](#manual-editing-and-conflict-resolution)).

User-local storage remains optional in `jp_storage`. When it is `None` (e.g., a
headless server setup, or tests that don't need durability), the dual-write
logic is skipped and JP falls back to single-write workspace storage — the same
behavior as today. The `--local` flag and `local` derivation are unavailable
without user-local storage; all conversations are workspace-only.

> [!TIP]
> [RFD 039] adds tree-structured parent-child relationships via a `parent_id`
> field in each conversation's metadata, while retaining the flat directory
> layout described here.

### Shared User-Local Storage

Today, user-local storage is keyed by both the worktree directory name and
workspace ID: `~/.local/share/jp/workspace/<name>-<id>/`. This means each
worktree gets its own user-local silo, and removing a worktree orphans its silo.

This RFD changes the user-local path to be keyed by workspace ID only:

```
~/.local/share/jp/workspace/<workspace-id>/conversations/
```

All worktrees for the same repository share a single user-local store. This
aligns with [RFD 020], which already places session mappings and locks at
`~/.local/share/jp/workspace/<workspace-id>/`.

The `with_user_storage` method on `Storage` drops the `name` parameter. The
rename-on-mismatch logic in `with_user_storage` is replaced by a one-time
migration that moves existing `<name>-<id>` directories to `<id>`.

### The `local` Property

The `conversation.user` field (serialized as `"local"` in `metadata.json`) is
removed from the `Conversation` struct. The `local` indicator shown in `jp
conversation ls` becomes a runtime-derived property:

- **`local N`**: The conversation exists in the current workspace's
  `.jp/conversations/`. It is projected.
- **`local Y`**: The conversation exists only in user-local storage. It is not
  projected into this workspace.

This derivation is a filesystem check: does a directory matching this
conversation's ID exist in `<workspace>/.jp/conversations/`?

### Persistence Behavior

When JP persists a conversation:

1. Always write to user-local storage.
2. Check if a workspace copy exists (directory present in
   `.jp/conversations/`).
   - If yes, also write to workspace storage (update the projection).
   - If no, do not create a workspace projection.

For new conversations:

- `jp query --new` → create in both user-local and workspace.
- `jp query --new --local` → create in user-local only.

This means the workspace projection is created once (at conversation creation
time for non-local conversations, or explicitly via `jp conversation edit`), and
then maintained as long as the workspace directory survives. If the workspace
directory is deleted and recreated, the projection is gone, and the conversation
appears as `local Y` until explicitly re-projected.

### Toggling Projection

`jp conversation edit --local` toggles the workspace projection:

- **`local Y` → `local N`** (project into workspace): Copy the conversation from
  user-local to workspace `.jp/conversations/`.
- **`local N` → `local Y`** (remove from workspace): Delete the workspace copy.
  The user-local copy remains.

### Conversation Origin

When JP creates a conversation, it stores the worktree directory name as an
`origin` field in the conversation metadata. This is purely informational — it
has no behavioral impact — but provides context in `jp conversation ls` for
identifying which worktree a conversation came from, especially when viewing
conversations that were created in a worktree that no longer exists.

```json
{
  "origin": "feature-a",
  "last_activated_at": "2025-07-20T10:00:00.000Z"
}
```

The `origin` field is set once at creation time and never updated.

### Manual Editing and Conflict Resolution

JP uses plain, pretty-printed JSON files (`events.json`, `metadata.json`)
specifically so that users can edit them by hand. Tweaking a conversation's
context window — removing a noisy tool call, editing a response, trimming
history — is a supported workflow. The dual-write model must not break this.

The rule is **last-write-wins based on file modification time (mtime)**. When JP
loads a conversation that exists in both user-local and workspace storage, it
compares the mtime of the two `events.json` files (and separately, the two
`metadata.json` files). Whichever was modified more recently is the version JP
loads into memory.

On persist, JP writes to both locations, bringing them back into sync. After a
successful persist, both copies are identical with fresh mtimes.

The full load-persist cycle:

1. **Load**: For each conversation file (`events.json`, `metadata.json`), if
   both copies exist, compare mtimes. Load the newer one.
2. **Run**: Execute the query, tool calls, etc. The in-memory state reflects
   the user's edits.
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
3. Merge: conversations in user-local are the primary set. Conversations that
   exist only in the workspace (not in user-local) are included but marked as
   workspace-only (see [External Conversations](#external-conversations)).
4. Derive `local` for each conversation: check whether a workspace copy exists.

### External Conversations

Conversations can appear in the workspace that do not exist in user-local. This
happens when another contributor commits a conversation to git and you pull it.

These conversations are shown in `jp conversation ls` with a distinct indicator
(e.g., `origin: <their-worktree-name>` from the stored metadata, or simply the
absence of a user-local copy). They are read-only from JP's perspective until
the user interacts with them.

On first write interaction (`jp query --id=<id>`, `jp conversation edit`, or any
operation that modifies the conversation), the conversation is imported: copied
from workspace to user-local. From that point on, it follows the normal
dual-write persistence rules.

Non-write interactions (`jp conversation show`, `jp conversation print`, `jp
conversation ls`) read directly from the workspace without importing.

## Drawbacks

**Double disk I/O for non-local conversations.** Every persist operation writes
to two locations. For JSON conversation files this is negligible, but it is
strictly more work than the current single-write model.

**Divergence risk.** If JP crashes between writing to user-local and workspace
(or vice versa), the two copies can diverge. The mtime-based resolution (see
[Manual Editing and Conflict
Resolution](#manual-editing-and-conflict-resolution)) handles this gracefully —
whichever copy was written last is used on next load, and the next successful
persist re-syncs both copies. However, if the crash happens after writing
user-local but before writing workspace, the workspace copy may be stale until
the next JP run. Users who inspect workspace files directly between JP runs
could see outdated content.

**Conversation accumulation in user-local.** Because user-local is the durable
store, conversations accumulate there indefinitely (across worktree lifetimes).
The existing ephemeral conversation cleanup (`expires_at`) helps, but users who
create many conversations across many worktrees may eventually want a dedicated
cleanup command or policy. This is not new — user-local conversations already
accumulate today — but the volume increases when all conversations go through
user-local.

## Alternatives

### Keep current model, add evacuation command

A `jp conversation evacuate` command that moves all workspace conversations to
user-local before worktree removal. This requires the user to remember to run it
(or configure a git hook).

Rejected because it doesn't solve the problem — it shifts it from "remember to
use `--local`" to "remember to run evacuate." Forgetting either one results in
data loss.

### Store conversations at the bare repo level

For worktree setups, store conversations in a shared location above the
worktrees (e.g., `/path/to/project/.jp/conversations/`). All worktrees read from
and write to this shared directory.

Rejected because it requires JP to understand git worktree topology (resolving
the common git dir), which violates the goal of keeping JP git-unaware. It also
introduces write contention between concurrent worktrees.

### Sync conversations across workspaces

Keep per-worktree storage but replicate conversations bidirectionally across all
workspaces with the same ID.

Rejected because bidirectional sync is the wrong model. Conversations are
contextual to a branch or task — syncing a feature-a conversation into the
feature-b worktree is undesirable. The problem is durability, not distribution.

### `.jp` as a file (like `.git` in worktrees)

Make `.jp` a pointer file that redirects to shared storage, similar to how git
worktrees use a `.git` file pointing to the common git dir.

Rejected because `.jp/conversations/` needs to contain real files for `git
status` visibility. A pointer file would make conversations invisible to git,
defeating one of the two core requirements.

### Default all conversations to `--local`

Make `Conversation::default()` set `user: true`, so all conversations go to
user-local by default. Users who want git-visible conversations opt in
explicitly.

Rejected because it inverts the current expectation that conversations are
visible in the workspace by default. The dual-write approach preserves the
default behavior (workspace-visible) while adding durability.

## Non-Goals

- **Git awareness.** JP does not detect whether it is running inside a git
  worktree, does not resolve the common git directory, and does not read or
  write git-specific metadata. The design works for any workflow where the
  workspace directory might be deleted — worktrees, temporary clones, or manual
  cleanup.

- **Three-way merge conflict resolution.** If both copies are edited between JP
  runs (e.g., a user edits the workspace copy while a background process edits
  the user-local copy), the older edit is silently overwritten by the newer one.
  JP does not attempt to merge concurrent edits. In practice, users edit one
  copy (almost always the workspace one), so this is unlikely to cause issues.

- **Cross-machine sync.** User-local storage is local to the machine. This RFD
  does not address synchronizing conversations across machines.

- **Conversation garbage collection policy.** This RFD does not define when or
  how old conversations in user-local should be cleaned up. The existing
  `expires_at` mechanism continues to work. A dedicated cleanup UX is future
  work.

## Risks and Open Questions

### Migration of existing user-local directories

Renaming `<name>-<id>` directories to `<id>` needs to handle the case where
multiple worktrees have already created separate user-local directories (e.g.,
`main-otvo8` and `feature-a-otvo8`). The migration must merge their contents
without losing conversations. If two directories contain a conversation with the
same ID (unlikely but possible if both worktrees were used concurrently), the
most recently modified copy should win.

### Workspace copy freshness

The workspace copy is updated on every persist, but only if the workspace copy
already exists. If a user manually deletes a conversation from
`.jp/conversations/` (e.g., via `git checkout` or `git clean`), JP will not
recreate it — the conversation silently becomes local-only. This is arguably
correct behavior (the user deleted it), but might surprise users who expect
workspace conversations to be self-healing.

### Performance of `local` derivation

Deriving the `local` indicator requires checking the filesystem for each
conversation when listing. For workspaces with many conversations, this could
add latency to `jp conversation ls`. In practice, the number of conversations
per workspace is small (tens, not thousands), so this is unlikely to be a
problem. If it becomes one, the check can be batched into a single directory
listing.

## Implementation Plan

### Phase 1: Shared User-Local Storage

Change the user-local storage path from `<name>-<id>` to `<id>`. Add migration
logic to merge existing per-worktree directories. Update `with_user_storage` to
drop the `name` parameter.

Depends on: nothing.
Can be merged independently.

### Phase 2: Dual-Write Persistence

Change `persist_conversations_and_events` to always write to user-local. For
conversations where a workspace copy exists (or for new non-local
conversations), also write to the workspace. Remove the `conversation.user`
field from the `Conversation` struct. Derive the `local` property at runtime
from workspace filesystem state.

Depends on: Phase 1.
Can be merged independently.

### Phase 3: Conversation Origin Metadata

Add the `origin` field to `Conversation`, populated from the worktree directory
name at creation time. Surface it in `jp conversation ls` and `jp conversation
show`.

Depends on: nothing (can be done in parallel with Phase 1/2).
Can be merged independently.

### Phase 4: External Conversation Import

Add lazy-import logic: when JP encounters a conversation in the workspace that
does not exist in user-local, and the user performs a write operation on it,
copy it to user-local first. Update `jp conversation ls` to display
workspace-only conversations with appropriate indicators.

Depends on: Phase 2.
Can be merged independently.

### Phase 5: Update `jp conversation edit --local`

Change `--local` toggling to copy-to-workspace / delete-from-workspace instead
of the current move-between-storage-locations behavior.

Depends on: Phase 2.
Can be merged independently.

## References

- [RFD 020: Parallel Conversations][RFD 020] — removes `active_conversation_id`,
  introduces per-session conversation tracking and conversation locks. This
  RFD's shared user-local storage aligns with RFD 020's session and lock storage
  paths.

[RFD 020]: 020-parallel-conversations.md
[RFD 039]: 039-conversation-trees.md
