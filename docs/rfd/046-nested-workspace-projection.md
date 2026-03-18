# RFD 046: Nested Workspace Projection for Conversation Trees

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-16
- **Extends**: [RFD 039](039-conversation-trees.md)

## Summary

This RFD extends [RFD 039]'s conversation tree design by changing the workspace
storage layout from flat to nested. User-local storage remains flat (as [RFD
031] describes). Workspace storage projects conversations as a nested directory
tree derived from `parent_id`, giving filesystem visibility to the tree
hierarchy. `ls .jp/conversations/` shows only root conversations. `rm -r` on a
parent cleans up its children. Sub-agent conversations are naturally grouped
under their parent's directory.

All tree features from [RFD 039] — `parent_id`, tree index, fork-as-child,
`conversation ls --tree`, `conversation rm --cascade/--promote`, workspace API —
are unchanged. This RFD only addresses how conversations are laid out on disk in
workspace storage, and the sync and projection behaviors that follow from that
layout.

## Motivation

[RFD 039] stores all conversations flat in both user-local and workspace. The
tree is encoded in `parent_id` metadata and visible through `jp conversation ls
--tree`, but invisible in the filesystem. This works but misses the tangible UX
benefits of nested directories:

- `ls .jp/conversations/` shows every conversation including sub-agent work.
  With 10+ sub-agent conversations per parent ([RFD 040]), the top-level
  directory becomes noisy.
- `rm -r <parent>` in the workspace doesn't clean up children — they're separate
  top-level directories.
- `git status` shows a flat list of conversation directories with no grouping.
- File browsers don't reveal the hierarchy.
- Browsing conversations on git hosting platforms (GitHub, GitLab) shows a flat
  list of opaque ID directories. The tree structure is invisible without access
  to `jp conversation ls --tree`, which isn't available when reviewing a
  repository on the web.

A nested workspace layout solves these by making the tree structure visible and
actionable through standard filesystem tools and web UIs.

User-local storage stays flat. Its job is durability — surviving workspace
destruction ([RFD 031]). Flat is ideal for that job: simple, robust, no tree
operations can accidentally cascade into data loss.

## Design

### Workspace storage layout

Workspace storage projects conversations as a nested tree:

```txt
.jp/conversations/
  ROOT_A/
    events.json
    metadata.json
    conversations/
      CHILD_B/
        events.json
        metadata.json
        conversations/
          GRANDCHILD_C/
            events.json
            metadata.json
  ROOT_D/
    events.json
    metadata.json
```

The nesting is derived from `parent_id` metadata ([RFD 039]). Root conversations
live directly under `.jp/conversations/`. Children are nested under their
parent's `conversations/` subdirectory. The workspace tree is a **projection** —
it can always be rebuilt from user-local data.

### User-local storage layout

User-local storage remains flat, as [RFD 031] describes:

```txt
~/.local/share/jp/workspace/<workspace-id>/conversations/
  ROOT_A/
    events.json
    metadata.json
  CHILD_B/
    events.json
    metadata.json
  GRANDCHILD_C/
    events.json
    metadata.json
  ROOT_D/
    events.json
    metadata.json
```

Every conversation lives directly under `conversations/`, regardless of its tree
position. No tree-aware logic is needed for the durable store.

### Workspace path computation

A conversation's workspace path is computed by walking its `parent_id` chain
using the in-memory tree index ([RFD 039]):

1. If `parent_id` is `None`, the workspace path is `.jp/conversations/<id>/`.
2. If `parent_id` is set, recursively compute the parent's workspace path, then
   append `conversations/<id>/`.

Example: C's parent is B, B's parent is A (a root):

```txt
.jp/conversations/A/conversations/B/conversations/C/
```

This computation is performed during persist (to write workspace copies) and
during load (to locate workspace copies for mtime comparison).

### Sync model

Each conversation is synced independently between its user-local path (flat) and
its workspace path (nested), using [RFD 031]'s mtime-based conflict resolution:

1. **Load**: For each conversation, compare mtimes of `events.json` (and
   separately `metadata.json`) between the flat user-local path and the computed
   nested workspace path. Load from the newer copy.
2. **Persist**: Write to user-local (flat, always). For projected conversations,
   compute the nested workspace path and write there.

The `conversations/` subdirectory of a workspace conversation is not part of the
sync comparison — children are synced independently.

### Persistence behavior

[RFD 031]'s persistence rules apply, with the workspace path being nested
instead of flat:

1. Always write to user-local storage (flat path).
2. Check if a workspace copy exists at the conversation's computed nested path.
   - If yes, also write to workspace (update the projection).
   - If no, do not create a workspace projection.

For new conversations:

- `jp query --new` - create in both user-local (flat) and workspace (nested
  under parent, if any).
- `jp query --new --local` - create in user-local only.

A refinement for tree support: on persist, a non-local conversation whose
workspace copy does not exist is projected to workspace **if its full ancestor
path exists in workspace**. This handles re-projection after a parent is toggled
back from local to non-local — children are automatically re-projected once
their ancestor path is viable again.

### `--local` toggle behavior

[RFD 039] notes that `--local` is independent per-conversation in the flat
layout. With nested workspace directories, the toggle gains cascade behavior:

**To local** (remove workspace projection): before deleting the workspace
directory, JP must sync the entire affected subtree to user-local. For the
target conversation and every descendant whose workspace copy will be destroyed
by the cascade:

1. Load its files from both locations (triggering the mtime comparison).
2. If the workspace copy is newer, write it to user-local immediately.

Only after all descendants are synced does JP delete the conversation's
workspace directory. This is necessary because conversation events are
lazy-loaded (`OnceCell`) — a descendant's events may not be in memory when the
workspace directory is removed. Without the eager sync, manual edits to a
child's workspace `events.json` would be lost when a parent is toggled to local.

Because children are nested inside the parent's workspace directory, the
deletion cascades **down** — all descendants lose their workspace projections.
No data is lost: user-local has been synced first.

**To non-local** (create workspace projection): JP creates the workspace copy at
the conversation's computed nested path. This requires all ancestors to have
workspace directories, so it cascades **up** — any local-only ancestor is also
projected to workspace. Before projecting each ancestor, JP loads it first
(syncing any newer workspace content to user-local).

When a parent is toggled back from local to non-local, its non-local children
are automatically re-projected on the next persist (see [Persistence
behavior](#persistence-behavior)).

The toggle should report the number of descendant conversations affected by the
cascade.

### Workspace projection maintenance

On persist, after writing all conversations to their workspace paths, JP cleans
up stale workspace directories. A workspace directory is stale if:

- It contains a conversation ID that no longer exists (was deleted).
- It is at a path that doesn't match the conversation's computed nested path
  (conversation was reparented or promoted).

The cleanup is a recursive walk of the workspace tree, comparing found
conversation IDs and paths against the in-memory tree index. Stale directories
are removed.

This handles:

- **Manual reparenting.** User edits `parent_id` in a workspace `metadata.json`.
  JP loads the newer metadata on the next run. On persist, the conversation is
  written at its new nested path, and the cleanup removes the old directory.
- **`--promote` operations.** Promoted conversations move to a new tree level.
  The old directories are cleaned up.

User-local requires no cleanup — it is flat and conversations are always
at `conversations/<id>/`.

### Interaction with RFD 031

[RFD 031]'s design is preserved with one structural change: workspace paths are
nested instead of flat. The per-conversation sync model applies without
modification — each conversation's files are independently compared by mtime
between its flat user-local path and its computed nested workspace path.

This separation means:

- **Durability is simple.** User-local is flat. No tree logic needed for the
  durable store. No risk of losing data through tree operations.
- **Workspace is rebuildable.** The nested tree is a projection. If the
  workspace is deleted (`git worktree remove`, `rm -rf .jp`), all data survives
  in user-local.
- **`--local` toggle is safe.** The eager subtree sync ensures all descendants'
  newer workspace content is written to user-local before the workspace
  directory is deleted (see [`--local` toggle
  behavior](#--local-toggle-behavior)).
- **External conversations work unchanged.** Pulled conversations arrive with
  their own `parent_id`. The tree is reconstructed from metadata on load. A
  `parent_id` can reference a non-existent conversation — the tree index treats
  missing parents as roots.
- **Reparenting propagates through git.** A committed `parent_id` change
  restructures the workspace tree on the next persist.

## Drawbacks

**Two different layouts.** User-local and workspace have different directory
structures. This adds complexity in the persist/load layer — workspace path
computation (walking the `parent_id` chain) and workspace projection maintenance
(stale directory cleanup) are new code. However, the complexity is contained in
`jp_storage`. The rest of the codebase works with conversation IDs and the
in-memory tree index, not paths.

**Workspace projection cleanup cost.** Each persist includes a recursive walk of
the workspace tree to find stale directories. For workspaces with many
conversations, this adds I/O. In practice the walk is bounded by conversation
count (tens to low hundreds). The cleanup can be skipped when no structural
changes occurred by tracking a dirty flag.

**`--local` cascade on children.** Toggling a parent to local removes children's
workspace projections. The children are re-projected when the parent is toggled
back, but a user who toggles a parent to local may not realize they've also
hidden descendant conversations from the workspace, although workspaces are
usually VCS-backed, so it would show up in e.g. `git status`.

**Workspace path depth.** Deeply nested conversations create deep workspace
paths. Each nesting level adds ~25 characters (conversation ID + the
`conversations/` segment). Even 10 levels adds ~250 characters, well within
modern filesystem limits.

## Alternatives

### Nested storage in both locations

Use nested directories in both user-local and workspace. Either derive
parent-child from directory structure, or use `parent_id` metadata with
bidirectional sync.

Rejected because it couples tree structure to durability. User-local's job is
being the copy that survives workspace destruction — flat is ideal for that.
Nesting in user-local adds complexity (sync manifest for deletion detection,
bidirectional reconciliation) without a concrete benefit, since users rarely
interact with user-local directly.

If a future feature requires nested user-local, this can be added by introducing
a sync manifest at that point. The flat user-local design does not close this
door.

### Keep flat workspace (no change from RFD 039)

Keep the flat workspace layout from [RFD 039]. The tree is only visible through
`jp conversation ls --tree`.

Rejected because it misses the filesystem UX that makes trees valuable in
practice: sub-agent grouping, natural cleanup, git status visibility. Especially
with [RFD 040]'s sub-agent conversations, a flat workspace becomes noisy.

## Non-Goals

- **Bidirectional tree sync.** User-local is flat. Workspace nesting is a
  one-way projection. There is no bidirectional sync of directory structure.

- **Nested user-local storage.** User-local remains flat. This RFD does not add
  nesting to user-local.

## Risks and Open Questions

### Partial trees from independent projection

[RFD 039] notes that a child can be projected while its parent is local-only,
creating partial trees for team members who pull from git. With nested workspace
directories, this situation is structurally prevented for the cascading
direction: toggling a parent to local removes children's workspace copies. But a
child can still be explicitly made non-local while its parent is local, if the
child was created after the parent was toggled to local. In that case, the child
can't be projected (no ancestor path), so it remains local-only until the parent
is re-projected.

The risk from [RFD 039] about future hard dependencies between parent and child
still applies — the dependent feature must handle missing parents gracefully.

### Workspace path stability

A conversation's workspace path changes when it or any ancestor is reparented.
Tools that cache workspace paths (e.g., editor bookmarks, shell history) may
break. This is inherent to any nested layout and is the same trade-off that git
worktrees and nested project structures make.

## Implementation Plan

All phases from [RFD 039] apply unchanged (parent_id, tree index, create with
parent, fork-as-child, ls --tree, rm strategies, garbage collection). This RFD
adds three additional phases that can be interleaved:

### Phase A: Nested workspace projection

Add workspace path computation from `parent_id` chain. Update
`persist_conversations_and_events` to write workspace copies at nested paths
instead of flat paths. Update load logic to find workspace copies at nested
paths for mtime comparison. Add workspace projection cleanup (recursive walk to
remove stale directories).

User-local storage remains flat — no changes to user-local paths.

Depends on [RFD 039] Phase 1 (parent_id and tree index).

### Phase B: `--local` cascade

Update `conversation edit --local` to load-then-delete when toggling to local
(ensuring newer workspace content is synced to user-local first). Implement
cascade down (remove workspace copies of descendants) and cascade up (project
ancestors when toggling to non-local). Report the number of affected
descendants. Implement the re-projection rule: on persist, non-local
conversations without workspace copies are projected if their full ancestor path
exists in workspace.

Depends on Phase A.

### Phase C: Workspace projection maintenance

Implement stale directory detection and cleanup during persist. Add dirty flag
to skip cleanup when no structural changes occurred.

Depends on Phase A. Can be merged alongside Phase B.

## References

- [RFD 020: Parallel Conversations][RFD 020] — defines `--fork[=N]`,
  conversation locks, and session identity.
- [RFD 031: Durable Conversation Storage][RFD 031] — dual-write persistence,
  mtime-based sync, and projection model.
- [RFD 038: Config Inheritance][RFD 038] — `--inherit` flag used by child
  conversations for config resolution.
- [RFD 039: Conversation Trees][RFD 039] — base design that this RFD extends.
  Defines the tree model, user-facing features, and workspace API.

[RFD 020]: 020-parallel-conversations.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 038]: 038-config-inheritance-for-new-conversations.md
[RFD 039]: 039-conversation-trees.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
