# RFD 046: Nested Workspace Projection for Conversation Trees

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-16
- **Extends**: [RFD 039]
- **Summary**: Projects conversation trees as nested directories in
  `.jp/conversations/` while keeping user-local storage flat for durability.

## Summary

This RFD extends [RFD 039]'s conversation tree design by projecting workspace
conversations (`.jp/conversations/`) as a nested directory tree derived from
`parent_id`, while keeping user-local storage flat for durability ([RFD 031]).
Root conversations sit at the top level of `.jp/conversations/`, and child or
sub-agent conversations nest under their parent's directory.
All tree semantics and CLI flags from [RFD 039] remain unchanged.

## Motivation

[RFD 039] stores all conversations flat in both user-local and workspace
storage.
The tree is encoded in `parent_id` metadata and visible through `jp conversation
ls --tree`, but invisible in the filesystem:

- `ls .jp/conversations/` shows every conversation, including sub-agent work.
  With 10+ sub-agent conversations per parent ([RFD 040]), the top-level
  directory becomes noisy.
- `rm -r <parent>` in the workspace leaves children behind as separate top-level
  directories.
- `git status` and file browsers show a flat list of conversation directories
  with no grouping.
- Git hosting platforms (GitHub, GitLab) show a flat list of opaque ID
  directories, and `jp conversation ls --tree` is not available when reviewing a
  repository on the web.

Nesting workspace directories exposes the tree structure to standard filesystem
tools and web UIs.

User-local storage stays flat because its purpose is durability across workspace
destruction ([RFD 031]).
A flat layout keeps the durable store simple and prevents tree operations from
cascading into data loss.

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

The nesting is derived from `parent_id` metadata ([RFD 039]).
Root conversations live directly under `.jp/conversations/`, and children live
under their parent's `conversations/` subdirectory.
Because the workspace tree is a **projection**, JP can always rebuild it from
user-local data.

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

Every conversation lives directly under `conversations/` regardless of its tree
position, so the durable store needs no tree-aware logic.

### Workspace path computation

JP computes a conversation's workspace path by walking its `parent_id` chain in
the in-memory tree index ([RFD 039]):

1. If `parent_id` is `None`, the workspace path is `.jp/conversations/<id>/`.
2. If `parent_id` is set, recursively compute the parent's workspace path and
   append `conversations/<id>/`.

For example, if C's parent is B and B's parent is root conversation A:

```txt
.jp/conversations/A/conversations/B/conversations/C/
```

JP computes this path on load to locate workspace copies for mtime comparison,
and on persist to write workspace copies.

### Sync model

Each conversation syncs independently between its flat user-local path and its
nested workspace path using [RFD 031]'s mtime-based conflict resolution:

1. **Load**: Compare the mtimes of `events.json` and `metadata.json`
   (separately) between the flat user-local path and the computed nested
   workspace path, and load from the newer copy.
2. **Persist**: Always write to the flat user-local path.
   For projected conversations, also write to the computed nested workspace
   path.

A workspace conversation's `conversations/` subdirectory is ignored when syncing
that conversation; children sync independently.

### Persistence behavior

[RFD 031]'s persistence rules apply at the computed nested workspace path:

1. Always write to the flat user-local path.
2. If a workspace copy already exists at the conversation's computed nested
   path, update it; otherwise, do not create a workspace projection.

For new conversations:

- `jp query --new`: create in both user-local (flat) and workspace (nested under
  its parent, if any).
- `jp query --new --local`: create in user-local only.

To support trees, persist also projects a non-local conversation without a
workspace copy **if its full ancestor path exists in the workspace**.
When a parent is toggled from local back to non-local, its non-local children
are automatically re-projected once their ancestor path exists again.

### `--local` toggle behavior

[RFD 039] notes that `--local` is independent per-conversation in the flat
layout.
With nested workspace directories, the toggle cascades:

**To local** (remove workspace projection): Because children nest inside their
parent's workspace directory, deleting a parent's workspace directory cascades
**down** and removes the workspace projections of all its descendants.

Because conversation events are lazy-loaded (`OnceCell`), a descendant's events
may not be in memory when the workspace directory is removed.
To avoid losing manual edits to a child's workspace `events.json`, JP syncs the
entire affected subtree to user-local before deleting the workspace directory.
For the target conversation and every projected descendant, JP:

1. Loads its files from both locations (triggering the mtime comparison).
2. Writes the workspace copy to user-local immediately if the workspace copy is
   newer.

After syncing all descendants, JP deletes the conversation's workspace
directory.

**To non-local** (create workspace projection): JP creates the workspace copy at
the conversation's computed nested path.
Because a nested path requires every ancestor directory to exist in the
workspace, this cascades **up**: any local-only ancestor is also projected to
the workspace.
Before projecting each ancestor, JP loads it to sync any newer workspace content
to user-local.

When a parent is toggled back from local to non-local, its non-local children
are automatically re-projected on the next persist (see [Persistence
behavior](#persistence-behavior)).

The toggle reports the number of descendant conversations affected by the
cascade.

### Workspace projection maintenance

On persist, after writing all conversations to their workspace paths, JP
recursively walks the workspace tree, compares the discovered conversation IDs
and paths against the in-memory tree index, and removes stale directories.
A workspace directory is stale if:

- Its conversation ID no longer exists (the conversation was deleted).
- Its path does not match the conversation's computed nested path (the
  conversation was reparented or promoted).

This cleanup handles two cases:

- **Manual reparenting**: A user edits `parent_id` in a workspace
  `metadata.json`.
  JP loads the newer metadata on the next run, writes the conversation to its
  new nested path on persist, and removes the old directory.
- **`--promote` operations**: Promoted conversations move to a new tree level,
  and cleanup removes their old directories.

User-local storage requires no cleanup because it is flat and conversations
always live at `conversations/<id>/`.

### Interaction with RFD 031

[RFD 031]'s design is preserved with one structural change: workspace paths are
nested instead of flat.
The per-conversation sync model applies unchanged, comparing each conversation's
files by mtime between its flat user-local path and its computed nested
workspace path.

This separation has several properties:

- **Simple durability**: User-local storage is flat, so the durable store needs
  no tree logic and tree operations cannot cause data loss.
- **Rebuildable workspace**: The nested tree is a projection.
  If the workspace is deleted (`git worktree remove`, `rm -rf .jp`), all data
  survives in user-local storage.
- **Safe `--local` toggle**: Eager subtree sync writes any newer workspace
  content from descendants to user-local before deleting the workspace directory
  (see [`--local` toggle behavior](#local-toggle-behavior)).
- **Unchanged external conversations**: Pulled conversations arrive with their
  own `parent_id`, and JP reconstructs the tree from metadata on load.
  If `parent_id` references a missing conversation, the tree index treats the
  conversation as a root.
- **Git-propagated reparenting**: A committed `parent_id` change restructures
  the workspace tree on the next persist.

## Drawbacks

**Two directory layouts.** User-local and workspace storage use different
directory structures, which adds workspace path computation (walking the
`parent_id` chain) and stale-directory cleanup to the persist/load layer.
This complexity stays within `jp_storage`; the rest of the codebase works with
conversation IDs and the in-memory tree index rather than filesystem paths.

**Workspace projection cleanup cost.** Each persist walks the workspace tree
recursively to find stale directories.
This adds I/O in workspaces with many conversations, though the walk is bounded
by the conversation count (typically tens to low hundreds) and can be skipped
with a dirty flag when no structural changes occurred.

**`--local` cascade on children.** Toggling a parent to local removes its
children's workspace projections until the parent is toggled back.
A user may not expect toggling a parent to hide its descendants from the
workspace, though in a VCS-backed workspace the removed directories appear in
`git status`.

**Workspace path depth.** Each nesting level adds roughly 25 characters (the
conversation ID plus `conversations/`).
Even 10 levels add about 250 characters, well within modern filesystem limits.

## Alternatives

### Nested storage in both locations

Use nested directories in both user-local and workspace storage, either deriving
parent-child relationships from the directory structure or syncing `parent_id`
metadata bidirectionally.

This was rejected because it couples tree structure to durability.
User-local storage exists to survive workspace destruction, which a flat layout
handles simply.
Nesting user-local storage would require a sync manifest for deletion detection
and bidirectional reconciliation, with little benefit since users rarely inspect
user-local directories directly.
If a future feature requires nested user-local storage, a sync manifest can be
added then.

### Keep flat workspace (no change from RFD 039)

Keep the flat workspace layout from [RFD 039], where the tree is visible only
through `jp conversation ls --tree`.

This was rejected because it loses the filesystem benefits of a tree: sub-agent
grouping, recursive directory cleanup, and hierarchy in `git status` and web
UIs.
With [RFD 040]'s sub-agent conversations, a flat workspace directory quickly
becomes noisy.

## Non-Goals

- **Bidirectional tree sync.** Workspace nesting is a one-way projection from
  metadata; directory structure is not synced back to user-local storage.
- **Nested user-local storage.** User-local storage remains flat.

## Risks and Open Questions

### Partial trees from independent projection

[RFD 039] notes that projecting a child while its parent is local-only creates
partial trees for collaborators pulling from git.
Nested workspace directories prevent this when toggling a parent to local, since
the cascade removes the children's workspace copies.
However, if a child is created after its parent becomes local and is marked
non-local, JP cannot project the child until its ancestor path exists in the
workspace; the child stays in user-local storage until the parent is
re-projected.

As in [RFD 039], any future feature that introduces a hard dependency between
parent and child conversations must handle missing parents.

### Workspace path stability

A conversation's workspace path changes whenever it or an ancestor is
reparented, which can break external references such as editor bookmarks or
shell history.
This trade-off is inherent to encoding hierarchy in filesystem paths, just as in
git worktrees and nested project directories.

## Implementation Plan

All phases from [RFD 039] apply unchanged (`parent_id`, tree index, create with
parent, fork-as-child, `ls --tree`, `rm` strategies, garbage collection).
This RFD adds three phases that can be interleaved:

### Phase A: Nested workspace projection

Compute workspace paths from the `parent_id` chain.
Update `persist_conversations_and_events` to write workspace copies at nested
paths instead of flat paths, and update load logic to locate workspace copies at
nested paths for mtime comparison.
Add a recursive walk during persist to remove stale workspace directories.
User-local storage paths remain flat and unchanged.

Depends on [RFD 039] Phase 1 (`parent_id` and tree index).

### Phase B: `--local` cascade

Update `conversation edit --local` to sync newer workspace content to user-local
before deleting workspace directories when toggling to local.
Implement cascade down (removing workspace copies of descendants) and cascade up
(projecting ancestors when toggling to non-local), and report the number of
affected descendants.
On persist, project any non-local conversation without a workspace copy whose
full ancestor path exists in the workspace.

Depends on Phase A.

### Phase C: Workspace projection maintenance

Detect and remove stale workspace directories during persist, gated by a dirty
flag that skips the walk when no structural changes occurred.

Depends on Phase A and can be merged alongside Phase B.

## References

- [RFD 020: Parallel Conversations][RFD 020]: defines `--fork[=N]`, conversation
  locks, and session identity.
- [RFD 031: Durable Conversation Storage][RFD 031]: dual-write persistence,
  mtime-based sync, and the projection model.
- [RFD 038: Config Reset Keywords][RFD 038]: `--inherit` flag used by child
  conversations for config resolution.
- [RFD 039: Conversation Trees][RFD 039]: base design that this RFD extends;
  defines the tree model, user-facing features, and workspace API.

[RFD 020]: 020-parallel-conversations.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 039]: 039-conversation-trees.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
