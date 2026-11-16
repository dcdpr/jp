# RFD 039: Conversation Trees

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08
- **Extended by**: [RFD 046]
- **Required by**: [RFD 051]
- **Summary**: Adds a `parent_id` metadata field to conversations for fork
  lineage and tree-structured `ls` and `rm` operations.

## Summary

This RFD organizes conversations into a parent-child hierarchy via a `parent_id`
field in `metadata.json` while keeping flat directory storage under
`.jp/conversations/`.
`conversation fork` creates a child conversation by default (with `--fork=0`
from [RFD 020] creating a blank child), `conversation ls` gains `--tree` and
`--root` flags, and `conversation rm` adds `--cascade` and `--promote`
strategies.

## Motivation

JP stores conversations as a flat list of peers with no structural relationship
between them, which creates two problems:

- **Forks have no lineage.** `conversation fork` copies events into a new
  top-level conversation with no reference to its source, so there is no way to
  tell which conversations were derived from another.
- **No organizational hierarchy.** Related conversations for a single task
  (explorations, sub-tasks, alternatives) accumulate in `conversation ls` as
  unrelated entries with no way to group them.

Organizing conversations into a tree solves both problems: forks become children
of their source, and related conversations can be grouped under a shared parent.
A parent-child hierarchy also supports features such as delegating tasks to
sub-agents or branching explorations.

> [!TIP]
> [RFD 040] is the first major consumer of conversation trees, organizing
> sub-agent conversations as hidden children under the main agent's
> conversation.

## Design

### Storage layout

All conversations live directly under `.jp/conversations/` in a flat directory
layout, regardless of their position in the tree:

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

The tree hierarchy is encoded in each conversation's metadata (see [Parent-child
relationship](#parent-child-relationship)) rather than in nested directories, so
existing conversations require no migration.

> [!TIP]
> [RFD 031] extends this storage layout with dual-write persistence to
> user-local storage.
> Because conversations remain flat, each conversation is synced independently
> between user-local and workspace storage, and the tree structure adds no sync
> complexity.

### Parent-child relationship

Each conversation records its parent in a `parent_id` field in `metadata.json`:

```json
{
  "title": "Alternative approach",
  "last_activated_at": "2026-03-08T10:00:00.000Z",
  "parent_id": "jp-c17528832001"
}
```

The `Conversation` struct gains:

```rust
/// The parent conversation ID, if this is a child conversation.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub parent_id: Option<ConversationId>,
```

Root conversations omit `parent_id` on serialization (`skip_serializing_if`),
and existing conversations without the field deserialize as roots (`parent_id:
None`).
Because `parent_id` lives in the conversation's own `metadata.json`, it travels
with the conversation through git, and reparenting a conversation only edits a
single file.

### Unbounded depth

Trees can nest to any depth: a child conversation can have children of its own
(for example, when a sub-agent delegates to another sub-agent, producing a
three-level hierarchy automatically).
Every tree operation supports arbitrary depth: `--promote` reparents children
onto the grandparent rather than the root, `--tree` indents each level under its
parent, and `--root=<id>` renders a subtree starting from any node.

Only two structural constraints apply:

- **Single parent.** A conversation has at most one parent (see
  [Non-Goals](#non-goals)), forming a tree rather than a general graph.
- **No cycles.** A `parent_id` cycle is invalid.
  Like a missing parent, a conversation reachable from itself is treated as a
  root by the tree index with a warning.

### Tree index

On workspace load, JP builds an in-memory tree index from the `parent_id` fields
of all loaded conversations and updates it incrementally when conversations are
created, removed, or reparented.
The index is derived state and is not persisted to disk.
It answers `parent_of`, `children_of`, and `has_children` queries without
rescanning metadata files.

Because `conversation ls` already loads all conversation metadata for titles,
event counts, and timestamps, building the tree index is a single pass over data
already in memory.

### Config inheritance

When a child conversation is created via `--fork=0` or `conversation fork`, [RFD
038]'s `--inherit` flag controls config inheritance.
The default (`--inherit=conversation`) inherits the parent's resolved config.

### Creating child conversations

`jp query --fork=0` ([RFD 020]) creates a blank child conversation with no
copied event history (0 turns) and `parent_id` set to the parent's ID:

```sh
jp query --fork=0 "message"
jp query --fork=0 --id=<parent-id> "message"
```

Without `--id`, the parent is the session's active conversation; with `--id`,
the parent is `<parent-id>`.
To copy recent turns into the new child conversation, pass `--fork[=N]` with a
larger `N` ([RFD 020]).

### Fork as a child conversation

`conversation fork` creates a child of the source conversation rather than a new
root conversation:

```sh
jp conversation fork <id>
```

The new conversation sets `parent_id` to `<id>` and copies the selected events.
Time-based and count-based turn selectors (`--from`, `--to`, `--turn`,
`--first`, `--last`) filter which events are copied without affecting
`parent_id`, and `--activate` sets the new fork as the active conversation.

### `conversation ls` views

`conversation ls` gains `--root` and `--tree` flags to filter and format the
hierarchy.

By default, `conversation ls` prints a flat list of all conversations with a
`Root` column (`Y`/`N`) to distinguish roots from children:

```sh
jp conversation ls
```

Passing `--root` without a value lists only root conversations and omits the
redundant `Root` column (`--tree` is accepted here but has no effect because
children are excluded):

```sh
jp conversation ls --root
```

Passing `--root=<conversation-id>` lists the descendants of that conversation as
a flat list:

```sh
jp conversation ls --root=<conversation-id>
```

Passing `--tree` renders all conversations as a tree using box-drawing
characters in the table framework, with root conversations at the top level and
children indented under their parent:

```sh
jp conversation ls --tree
```

```txt
jp-c17528832001  Refactor error handling in jp_llm   3  5 mins ago
├── jp-c17528842001  [fork] Alternative approach     2  3 mins ago
└── jp-c17528852001  [fork] Original with tests      4  1 min ago
jp-c17528812001  Fix CI pipeline                     1  2 hours ago
```

Combining `--tree` with `--root=<conversation-id>` renders only that
conversation's subtree:

```sh
jp conversation ls --tree --root=<conversation-id>
```

```txt
jp-c17528832001  Refactor error handling in jp_llm    3  5 mins ago
├── jp-c17528842001  [fork] Alternative approach      2  3 mins ago
│   └── jp-c17528862001  Deeper exploration           5  4 mins ago
└── jp-c17528852001  [fork] Original with tests       4  1 min ago
```

### `conversation rm` with children

Removing a conversation that has children fails unless a removal strategy is
specified:

```sh
jp conversation rm <id>
# Error: Conversation jp-c17528832001 has 3 child conversations.
# Use --cascade to remove it and all its children.
# Use --promote to remove it and promote its children to root conversations.
```

- **`--cascade`** deletes the conversation and its entire subtree of
  descendants.
- **`--promote`** deletes the conversation and reparents its direct children
  onto its parent by setting each child's `parent_id` to the removed
  conversation's `parent_id` (or `None` if the removed conversation was a root).

The strategy flags are orthogonal to `--yes` (which skips confirmation prompts);
for example, `--cascade --yes` deletes the subtree without prompting.

### Workspace API changes

`Workspace::create_conversation` and `Workspace::remove_conversation` gain
optional parent and removal-strategy parameters:

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

`Workspace` also exposes three tree query methods backed by the in-memory tree
index:

```rust
/// Returns the parent conversation ID, read from the conversation's
/// metadata.
pub fn parent_of(&self, id: &ConversationId) -> Option<ConversationId>;

/// Returns all direct child conversation IDs for the given parent.
pub fn children_of(&self, parent_id: &ConversationId) -> Vec<ConversationId>;

/// Returns whether a conversation has any children.
pub fn has_children(&self, id: &ConversationId) -> bool;
```

### Path-shaped targeting

The tree index also provides the `parent_of` and root-traversal queries needed
for path-shaped `ConversationTarget` syntax (deferred to a separate change; see
[Non-Goals](#non-goals)):

| Target  | Meaning                                          |
| ------- | ------------------------------------------------ |
| `.`     | the session's active conversation (exists today) |
| `..`    | the parent of the active conversation            |
| `../..` | its grandparent, and so on for any depth         |
| `/`     | the root of the active conversation's tree       |

Segments compose against the resolved conversation, so applying `..` to a root
conversation returns an error naming that conversation rather than silently
resolving to the root itself.

This grammar is shared with workspace targeting ([RFD 087]), where the same
segments walk nested workspaces: `jp -w .` selects the current workspace from
any of its subdirectories, and `jp c show ..` selects the active conversation's
parent.
In both contexts, bare `.`, `..`, `../..`, and `/` are hierarchy keywords that
walk the workspace or conversation tree rather than the filesystem, whereas
filesystem paths include a named segment (`./foo`, `../foo`, `/foo/bar`).

### Interaction with RFD 020 (Parallel Conversations)

Conversation trees interact with [RFD 020]'s conversation locks and `--fork`
flag in two ways:

- **Independent locks.** Locking a conversation does not lock its children or
  parent.
  Each conversation has its own lock, so separate sessions can work on a parent
  and its child simultaneously.
- **`--fork` sets `parent_id`.** `--fork[=N]` creates the forked conversation
  with `parent_id` set to the source conversation and its own independent lock.

### Interaction with RFD 031 (Durable Conversation Storage)

[RFD 031] writes every conversation to user-local storage and syncs projected
(non-local) conversations to workspace storage via `mtime` comparison.
Because all conversations use a flat directory layout, each conversation's
`metadata.json` (including `parent_id`) syncs independently with no storage
coupling between parents and children:

- **Independent `--local` projection.** A child can be local-only while its
  parent is projected to the workspace, or vice versa, without cascading.
- **No orphan cleanup.** Conversations do not nest inside each other's
  directories, so deleting one conversation's workspace copy never affects
  another.
- **Missing parents are treated as roots.** When a team member pulls
  conversations from git, the tree index reconstructs the hierarchy from each
  conversation's `parent_id`.
  If a `parent_id` references a conversation that does not exist locally (for
  example, a local-only parent on another machine), the tree index treats the
  child as a root without requiring the ancestor chain to be imported.
- **Reparenting propagates through git.** Committing and pulling a `parent_id`
  change in `metadata.json` updates the tree structure for other team members.

## Drawbacks

- **Tree reconstruction cost.** Building the tree index requires loading all
  conversation metadata.
  This is negligible for workspaces with tens or hundreds of conversations
  because `conversation ls` already loads all metadata, and lazy `OnceCell`
  loading avoids reading metadata for thousands of conversations at startup.
- **Fork behavior change.** `fork` creates a child conversation instead of an
  independent root conversation.
  Forks still appear in `conversation ls` and can be activated directly, but
  tools or scripts that assume a flat conversation namespace may need updates.
- **Tree structure not visible on disk.** The flat `.jp/conversations/` layout
  does not show the hierarchy in a file browser; viewing the tree requires `jp
  conversation ls --tree` or reading `metadata.json` files directly.

## Alternatives

### Nested directory layout

Child conversations could be stored inside their parent's directory, deriving
the hierarchy from the filesystem layout:

```
.jp/conversations/
  PARENT_ID/
    events.json
    metadata.json
    conversations/
      CHILD_ID/
```

This was rejected because nesting complicates [RFD 031]'s dual-write persistence
model: user-local and workspace storage would have to mirror the same directory
tree, `--local` projection would cascade when removing a parent's workspace
copy, manual filesystem moves could leave orphaned duplicates requiring
recursive cleanup on every persist, and importing an external conversation would
require reconstructing its ancestor chain.
A flat layout avoids these problems because each conversation is synced,
projected, and imported independently.

### Symbolic links for tree structure

Parent-child relationships could be represented with symbolic links inside a
flat storage directory.
This was rejected because symlinks add Windows compatibility issues, break on
rename, and offer no advantage over a `parent_id` field in `metadata.json`.

### `--parent` flag instead of `--fork=0`

`jp query` could add a dedicated `--parent` flag for creating child
conversations.
This was not adopted because `--fork=0` ([RFD 020]) already creates a child
conversation that inherits the parent's config with 0 copied turns, avoiding a
second flag for the same operation.

### Centralized relationship file

All parent-child relationships could be stored in the top-level
`conversations/metadata.json` file instead of per-conversation `parent_id`
fields.
This was rejected because the top-level `metadata.json` is user-local only
(storing `active_conversation_id` and per-session state from [RFD 020]), whereas
per-conversation `parent_id` fields travel with each conversation through git.

## Non-Goals

- **Cross-tree references.** Each conversation has at most one parent and cannot
  appear in multiple trees.
- **Path-shaped targeting grammar.** [Path-shaped
  targeting](#path-shaped-targeting) documents the planned `..`, `../..`, and
  `/` syntax so the tree index supports it, but implementing those target
  keywords is a separate change.
- **Live config propagation.** Config inheritance copies the parent's config at
  child creation time ([RFD 038]); later changes to a parent's config do not
  propagate to existing children.
- **Conversation merge.** Merging two conversations into one (the inverse of
  `fork`) is out of scope.

## Risks and Open Questions

### Active conversation in a subtree

A session's active conversation ([RFD 020]) can be any root or child node in a
tree.
When the active conversation is a nested child, commands such as `jp query`
operate on it directly, which may surprise users who expect the active
conversation to be a root.
Showing the parent chain in the prompt or in `conversation show` mitigates this.

### Garbage collection of tree nodes

Removing an expired parent while its children are still live would leave
dangling `parent_id` references.
To prevent a parent from expiring before its descendants, a conversation's
effective expiration time is the maximum of its own `expires_at` and the
`expires_at` of its longest-lived descendant, computed by walking descendants in
the in-memory tree index.

### Partial trees from independent projection

When a projected child conversation references a local-only parent,
collaborators who pull from git do not have the parent conversation locally, and
the tree index treats the child as a root.
This works while parent-child links are purely organizational (tree views and
fork lineage), but if later features introduce runtime dependencies on the
parent (such as shared state or delegated tool access), projecting a child will
need to either require projecting its ancestors or handle missing parents
explicitly.

### Concurrent `parent_id` edits

If two collaborators reparent the same conversation on different branches,
merging produces a standard git conflict on `parent_id` in `metadata.json` (just
like conflicting edits to `title` or `expires_at`), which the user resolves
manually.

## Implementation Plan

### Phase 1: `parent_id` field and tree index

Add `parent_id` to `Conversation` with `#[serde(default, skip_serializing_if =
"Option::is_none")]` so existing conversations deserialize as roots with no
user-visible behavior change.
Build the in-memory tree index at workspace load time and add `parent_of`,
`children_of`, and `has_children` to `Workspace`.
This phase can be merged independently.

### Phase 2: `create_conversation` with parent

Update `Workspace::create_conversation` to accept an optional parent ID, set
`parent_id` on the new conversation, and update the in-memory tree index.
Depends on Phase 1.

### Phase 3: Fork as child conversation

Update `conversation fork` and `jp query --fork` ([RFD 020]) to set `parent_id`
to the source conversation's ID.
Depends on Phase 2.

### Phase 4: `conversation ls` tree view

Add `--tree` and `--root` to `conversation ls`, render trees with box-drawing
characters in the table framework, and add the `Root` column to the default flat
view.
Depends on Phase 1.

### Phase 5: `conversation rm` strategies

Check `has_children` in `Workspace::remove_conversation`, implement `--cascade`
and `--promote`, and wire both flags into `conversation rm`.
Depends on Phase 1.

### Phase 6: Garbage collection update

Update `expires_at` garbage collection to walk the in-memory tree index so a
parent's effective expiration is the maximum of its own `expires_at` and its
descendants'.
Depends on Phase 1.

## References

- [RFD 020: Parallel Conversations][RFD 020]: defines `--fork[=N]`, conversation
  locks, and session identity.
- [RFD 031: Durable Conversation Storage][RFD 031]: dual-write persistence,
  `mtime`-based sync, and workspace projection.
- [RFD 038: Config Reset Keywords][RFD 038]: `--inherit` flag used by child
  conversations for config resolution.
- `crates/jp_workspace/src/lib.rs`: workspace implementation.
- `crates/jp_storage/src/lib.rs`: storage implementation.
- `crates/jp_conversation/src/conversation.rs`: `Conversation` struct.
- `crates/jp_cli/src/cmd/conversation/fork.rs`: `conversation fork`
  implementation.
- `crates/jp_cli/src/cmd/conversation/ls.rs`: `conversation ls` implementation.
- `crates/jp_cli/src/cmd/conversation/rm.rs`: `conversation rm` implementation.

[RFD 020]: 020-parallel-conversations.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 040]: 040-hidden-conversations-and-tool-context.md
[RFD 046]: 046-nested-workspace-projection-for-conversation-trees.md
[RFD 051]: 051-sub-agent-workflows-with-local-tools.md
[RFD 087]: 087-session-scoped-active-workspace.md
