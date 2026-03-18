# RFD 039: Conversation Trees

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08
- **Extended by**: [RFD 046](046-nested-workspace-projection.md)

## Summary

This RFD introduces a tree-structured conversation hierarchy. Every conversation
can have child conversations. The parent-child relationship is stored as a
`parent_id` field in each conversation's `metadata.json`, while all
conversations remain in a flat directory layout under `.jp/conversations/`.
`conversation ls` gains `--tree` and `--root` flags. `conversation rm` supports
`--cascade` and `--promote` strategies when a conversation has children.
`conversation fork` creates a child instead of a top-level copy, and `--fork=0`
([RFD 020]) becomes the mechanism for creating blank child conversations.

## Motivation

JP's conversations are currently a flat list. Every conversation is a peer —
there is no structural relationship between them. This creates two problems:

**Forks have no lineage.** `conversation fork` creates a new top-level
conversation by copying events. The fork has no reference to its source. Over
time, the relationship is lost. You can't answer "what conversations were
derived from this one?"

**No organizational hierarchy.** Users working on complex tasks often create
multiple related conversations (exploration, sub-tasks, alternatives). These
pile up in `conversation ls` as unrelated entries. There is no way to group
them.

A tree model solves both: forks are children of their source, and related
conversations can be grouped under a parent. The tree also provides a natural
foundation for future features that need parent-child conversation
relationships, such as delegating tasks to sub-agents or branching explorations.

> [!TIP]
> [RFD 040] is the first major consumer of conversation trees, organizing
> sub-agent conversations as hidden children under the main agent's
> conversation.

## Design

### Storage layout

All conversations are stored in a flat directory structure, unchanged from
today:

```txt
.jp/conversations/
  PARENT_ID/
    events.json
    metadata.json
  CHILD_ID_1/
    events.json
    metadata.json
  CHILD_ID_2/
    events.json
    metadata.json
  GRANDCHILD_ID/
    events.json
    metadata.json
```

No nesting. Every conversation lives directly under `.jp/conversations/`,
regardless of its position in the tree. The tree structure is encoded in each
conversation's metadata (see [Parent-child
relationship](#parent-child-relationship)), not in the directory hierarchy.

Existing conversations are root conversations — they have `parent_id: null` (or
absent, via serde default). No migration is needed.

> [!TIP]
> [RFD 031] extends this storage layout with dual-write persistence to
> user-local storage. Because conversations remain flat, each conversation is
> independently synced between user-local and workspace storage — the tree
> structure introduces no additional sync complexity.

### Parent-child relationship

The parent-child relationship is stored as a `parent_id` field in each
conversation's `metadata.json`:

```json
{
  "title": "Alternative approach",
  "last_activated_at": "2026-03-08T10:00:00.000Z",
  "parent_id": "jp-c17528832001"
}
```

Root conversations have no `parent_id` field (omitted via serde's
`skip_serializing_if`). Existing conversations deserialize with `parent_id:
None` via serde defaults.

The `Conversation` struct gains:

```rust
/// The parent conversation ID, if this is a child conversation.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub parent_id: Option<ConversationId>,
```

Because `parent_id` lives in the conversation's own `metadata.json`, it travels
with the conversation through git — team members who pull a conversation also
get its parent reference. Reparenting a conversation is an edit to a single
file.

### Tree index

On workspace load, JP builds an in-memory tree index from the `parent_id` fields
of all loaded conversations. This index supports `parent_of`, `children_of`, and
`has_children` queries without scanning metadata files on each call.

The index is rebuilt on load and updated incrementally when conversations are
created, removed, or reparented. It is not persisted — it is derived state.

`conversation ls` already loads all conversation metadata (for titles, event
counts, timestamps). Building the tree index is an additional pass over data
that is already in memory.

### Config inheritance

When a child conversation is created (via `--fork=0` or `conversation fork`),
config inheritance is handled by [RFD 038]'s `--inherit` flag. The default
behavior (`--inherit=conversation`) inherits the parent's resolved config, which
is the natural choice for child conversations.

### Creating child conversations

Child conversations are created using `--fork=0` from [RFD 020]:

```sh
jp query --fork=0 "message"
jp query --fork=0 --id=<parent-id> "message"
```

`--fork=0` creates a child conversation that inherits the parent's config but
copies no event history (0 turns). Without `--id`, the parent is the session's
active conversation. With `--id`, the parent is the specified conversation.

The new conversation's `parent_id` is set to the parent's ID.

For creating a child with some event history, use `--fork[=N]` with a larger N,
as defined in [RFD 020].

### Fork as a child conversation

`conversation fork` changes from creating a top-level copy to creating a child
of the source conversation:

```sh
jp conversation fork <id>
```

Creates a child conversation with `parent_id` set to `<id>` and copied events
(filtered by `--from`, `--until`, `--last` as today).

The `--activate` flag continues to work — it sets the fork as the active
conversation.

### `conversation ls` views

Two new flags control the listing:

```sh
jp conversation ls
```

Flat list of all conversations. Includes a `Root` column (`Y`/`N`) to
distinguish roots from children.

```sh
jp conversation ls --root
```

Flat list of root conversations only. No `Root` column (redundant since all
shown conversations are roots). `--tree` is accepted but has no effect (roots
have no visible children in this view).

```sh
jp conversation ls --root=<conversation-id>
```

Flat list of conversations that are descendants of the specified conversation.
Combine with `--tree` to see the subtree structure.

```sh
jp conversation ls --tree
```

Tree view of all conversations, rendered using the existing table framework with
box-drawing characters:

```txt
jp-c17528832001  Refactor error handling in jp_llm   3  5 mins ago
├── jp-c17528842001  [fork] Alternative approach     2  3 mins ago
└── jp-c17528852001  [fork] Original with tests      4  1 min ago
jp-c17528812001  Fix CI pipeline                     1  2 hours ago
```

Root conversations are listed at the top level. Children are indented under
their parent.

```sh
jp conversation ls --tree --root=<conversation-id>
```

Tree view of a specific subtree:

```txt
jp-c17528832001  Refactor error handling in jp_llm    3  5 mins ago
├── jp-c17528842001  [fork] Alternative approach      2  3 mins ago
│   └── jp-c17528862001  Deeper exploration           5  4 mins ago
└── jp-c17528852001  [fork] Original with tests       4  1 min ago
```

### `conversation rm` with children

Removing a conversation that has children requires specifying a strategy:

```sh
jp conversation rm <id>
# Error: Conversation jp-c17528832001 has 3 child conversations.
# Use --cascade to remove it and all its children.
# Use --promote to remove it and promote its children to root conversations.
```

Two strategies:

**`--cascade`**: Removes the conversation and all its descendants. This is a
destructive operation — the entire subtree is deleted.

**`--promote`**: Removes the conversation but promotes its direct children to
the grandparent (or root if there is no grandparent). Each promoted child's
`parent_id` is updated to the removed conversation's `parent_id` (or set to
`None` if the removed conversation was a root).

Without a strategy flag, removal of a conversation with children is refused.

The existing `--yes` flag (skip confirmation prompt) is orthogonal to the
strategy flags. `--cascade --yes` deletes the tree without prompts.

### `conversation fork` with `--from` / `--until` / `--last`

The current fork command supports time-based and count-based event filtering.
These continue to work: the fork creates a child conversation with a filtered
copy of the parent's events. The child's `parent_id` points to the source
regardless of filtering.

### Workspace API changes

The existing `create_conversation` and `remove_conversation` methods gain
optional parent and strategy handling:

```rust
/// Creates a new conversation, optionally as a child of the given parent.
pub fn create_conversation(
    &mut self,
    parent: Option<&ConversationId>,
    conversation: Conversation,
    config: Arc<AppConfig>,
) -> Result<ConversationId>;

/// Removes a conversation.
///
/// If the conversation has children, `strategy` determines what happens:
/// - `Cascade`: remove the conversation and all descendants.
/// - `Promote`: remove the conversation and promote children to the
///   grandparent (or root).
///
/// Returns an error if the conversation has children and no strategy
/// is specified.
pub fn remove_conversation(
    &mut self,
    id: &ConversationId,
    strategy: Option<RemovalStrategy>,
) -> Result<Option<Conversation>>;
```

```rust
pub enum RemovalStrategy {
    /// Remove the conversation and all descendants.
    Cascade,
    /// Remove the conversation, promote children to grandparent (or root).
    Promote,
}
```

Additional query methods:

```rust
/// Returns the parent conversation ID, read from the conversation's
/// metadata.
pub fn parent_of(&self, id: &ConversationId) -> Option<ConversationId>;

/// Returns all direct child conversation IDs for the given parent.
pub fn children_of(&self, parent_id: &ConversationId) -> Vec<ConversationId>;

/// Returns whether a conversation has any children.
pub fn has_children(&self, id: &ConversationId) -> bool;
```

These methods read from the in-memory tree index, not from disk.

### Interaction with RFD 020 (Parallel Conversations)

[RFD 020] introduces conversation locks and per-session conversation tracking.
The tree structure interacts in two ways:

**Locking scope.** A lock on a conversation does NOT lock its children. Each
conversation has its own independent lock. Two sessions can work on a parent and
child simultaneously.

**`--fork` creates children.** RFD 020's `--fork[=N]` creates a new conversation
by copying events from the source. With trees, the fork has `parent_id` set to
the source conversation. The fork gets its own independent lock.

### Interaction with RFD 031 (Durable Conversation Storage)

[RFD 031] introduces dual-write persistence: every conversation is always
written to user-local storage, and projected (non-local) conversations are
additionally written to workspace storage.

Because all conversations remain in a flat directory layout, RFD 031's
per-conversation sync model applies without modification. Each conversation's
`metadata.json` (which contains `parent_id`) is synced independently between
user-local and workspace via mtime comparison. There is no structural coupling
between a parent's storage and its children's storage.

This means:

- **`--local` is independent per-conversation.** A child can be local-only while
  its parent is projected to workspace, or vice versa. No cascade is needed.
- **No orphan cleanup.** Conversations don't live inside each other's
  directories, so deleting one conversation's workspace copy can't affect
  another.
- **External conversations work unchanged.** When a team member pulls
  conversations from git, each conversation arrives with its own `parent_id`.
  The tree is reconstructed from metadata on load. No ancestor chain needs to be
  imported for the tree to be valid — a `parent_id` can reference a conversation
  that doesn't exist locally (the parent might be local-only on the other team
  member's machine). The tree index treats missing parents as roots.
- **Reparenting propagates through git.** If a team member changes a
  conversation's `parent_id` and commits the change, pulling it updates the tree
  structure for everyone.

## Drawbacks

**Tree reconstruction cost.** Building the tree index requires loading all
conversation metadata. For a workspace with tens to low hundreds of
conversations, this is negligible — `conversation ls` already loads all
metadata. For workspaces with thousands of conversations (unlikely in practice),
the lazy `OnceCell` loading pattern ensures metadata is read from disk on
demand, not all at startup.

**Fork behavior change.** Existing users expect `fork` to create an independent
top-level conversation. After this change, forks are children. They still appear
in `conversation ls` and can be activated, so the user-visible behavior is
similar. But tools or scripts that assume a flat conversation namespace may need
adjustment.

**Tree structure not visible on disk.** Unlike a nested directory layout, the
flat structure doesn't reveal the tree hierarchy in a file browser. The tree is
only visible through `jp conversation ls --tree` or by inspecting individual
`metadata.json` files. In practice this is fine — the workspace directory is
primarily for git visibility of conversation contents, not their hierarchy.

## Alternatives

### Nested directory layout

Store child conversations inside the parent's directory:

```
.jp/conversations/
  PARENT_ID/
    events.json
    metadata.json
    conversations/
      CHILD_ID/
```

Derive the parent-child relationship from the directory structure.

Rejected because the nested layout creates significant complexity when
combined with [RFD 031]'s dual-write persistence model. Both user-local and
workspace storage must maintain identical tree structures, projection status
(`--local`) cascades through the tree (removing a parent's workspace copy
removes all children's copies), manual filesystem operations can create
orphaned duplicates requiring recursive cleanup on every persist, and
external conversation import must reconstruct ancestor chains. The flat
layout avoids all of these issues — each conversation is independently
synced, independently projected, and independently importable.

### Symbolic links for tree structure

Keep a flat storage directory but create symbolic links to represent
parent-child relationships.

Rejected because symlinks add platform-specific complexity (Windows
compatibility), are fragile (break on rename), and don't provide meaningful
benefits over a metadata field.

### `--parent` flag instead of `--fork=0`

A dedicated `--parent` flag on `jp query` for creating child conversations.

Not adopted because `--fork=0` from [RFD 020] achieves the same thing: it
creates a child conversation that inherits config but has no event history.
Reusing `--fork` avoids a new flag and keeps the concepts unified — a fork
with 0 turns is conceptually a blank child.

### Centralized relationship file

Store all parent-child relationships in the top-level
`conversations/metadata.json` instead of per-conversation `parent_id`.

Rejected because the top-level `metadata.json` is user-local only (it holds
`active_conversation_id` and per-session state from [RFD 020]). Tree
structure needs to travel with conversations through git so that team members
who pull conversations also get the hierarchy. Per-conversation `parent_id`
achieves this — each conversation is self-contained.

## Non-Goals

- **Cross-tree references.** A conversation can only be a child of one
  parent. Having a conversation appear in multiple trees is not supported.

- **Tree-level config overrides.** Config inheritance flows from parent to
  child at creation time (via [RFD 038]). There is no mechanism to change a
  parent's config and have it propagate to existing children.

- **Conversation merge.** Combining two conversations into one (reverse of
  fork) is not in scope.

## Risks and Open Questions

### Active conversation in a subtree

The active conversation (per [RFD 020], the session's default conversation)
can be any conversation in the tree — root or child. When the active
conversation is a deeply nested child, commands like `jp query` operate on it
directly. This is correct but may surprise users who expect the active
conversation to always be a root. Clear indication in the prompt or
`conversation show` (showing the parent chain) mitigates this.

### Garbage collection of tree nodes

The existing `expires_at` mechanism removes expired conversations. With
trees, an expired parent with non-expired children creates a dangling
reference situation. Policy: a parent's effective expiration time is the
maximum of its own `expires_at` and the `expires_at` of its longest-lived
descendant. A parent cannot expire before all of its children have expired.
This prevents dangling references by construction.

The expiration check requires building the tree (which is already available
via the in-memory index) and computing the effective expiration by walking
descendants.

### Partial trees from independent projection

Because `--local` is independent per-conversation, a child can be projected
to workspace while its parent is local-only. Team members who pull that
child from git will have a `parent_id` referencing a conversation that
doesn't exist on their machine. The tree index handles this gracefully —
the child appears as a root.

This is acceptable for the current design, where parent-child relationships
are purely organizational (tree views, fork lineage). However, if future
work introduces hard dependencies between parent and child (shared state,
inherited runtime config, delegated tool access), a missing parent would
break those features. At that point, projecting a child may need to require
that its ancestors are also projected, or the dependent feature must
explicitly handle the missing-parent case.

### Concurrent `parent_id` edits

Two team members could reparent the same conversation on different branches.
When git merges, the `metadata.json` conflict is a normal JSON merge
conflict — the same as any other metadata field (title, expires_at). Git's
conflict markers surface the issue, and the user resolves it manually. JP
does not attempt automatic merge resolution.

## Implementation Plan

### Phase 1: `parent_id` field and tree index

Add `parent_id` to `Conversation`. Update serialization (serde default
`None`, skip if absent). Build the in-memory tree index at workspace load
time from all loaded conversation metadata. Add `parent_of`, `children_of`,
`has_children` to `Workspace`.

Existing conversations continue to work — they're roots with
`parent_id: None`.

Can be merged independently. No behavioral change for users.

### Phase 2: `create_conversation` with parent

Update `Workspace::create_conversation` to accept an optional parent ID.
When a parent is provided, set `parent_id` on the new conversation and
update the in-memory tree index.

Depends on Phase 1.

### Phase 3: Fork as child conversation

Update `conversation fork` to set `parent_id` to the source conversation's
ID. Update `--fork` in `jp query` ([RFD 020]) to set `parent_id` on the
new conversation.

Depends on Phase 2.

### Phase 4: `conversation ls` tree view

Add `--tree` and `--root` flags. Implement tree rendering using the existing
table framework with box-drawing characters. Add the `Root` column to the
default flat view.

Depends on Phase 1 (needs tree index).

### Phase 5: `conversation rm` strategies

Add `has_children` check to `remove_conversation`. Implement `--cascade`
(remove conversation and all descendants) and `--promote` (update children's
`parent_id` to grandparent, then remove). Wire the strategy flags.

Depends on Phase 1.

### Phase 6: Garbage collection update

Update the `expires_at` garbage collection to respect the tree: a parent's
effective expiration is the maximum of its own and its descendants'. Use the
in-memory tree index to walk descendants.

Depends on Phase 1.

## References

- [RFD 020: Parallel Conversations][RFD 020] — defines `--fork[=N]`,
  conversation locks, and session identity.
- [RFD 031: Durable Conversation Storage][RFD 031] — dual-write persistence,
  mtime-based sync, and projection model.
- [RFD 038: Config Inheritance][RFD 038] — `--inherit` flag used by child
  conversations for config resolution.
- `crates/jp_workspace/src/lib.rs` — current workspace implementation.
- `crates/jp_storage/src/lib.rs` — current storage implementation.
- `crates/jp_conversation/src/conversation.rs` — `Conversation` struct.
- `crates/jp_cli/src/cmd/conversation/fork.rs` — current fork implementation.
- `crates/jp_cli/src/cmd/conversation/ls.rs` — current listing implementation.
- `crates/jp_cli/src/cmd/conversation/rm.rs` — current removal implementation.

[RFD 020]: 020-parallel-conversations.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 038]: 038-config-inheritance-for-conversations.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
