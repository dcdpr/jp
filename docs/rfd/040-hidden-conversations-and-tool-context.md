# RFD 040: Hidden Conversations and Tool Context

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08

## Summary

This RFD adds two features to JP: a `hidden` metadata flag on conversations that
excludes them from default listings, and a `conversation_id` field in the tool
execution context. Together, these enable workflows where conversations are
created programmatically (by tools, scripts, or sub-agents) without cluttering
the user's conversation list, and where tools can reference the conversation
they're running in.

## Motivation

JP's conversation list is the primary way users navigate their work. As usage
patterns grow more sophisticated — forking conversations for experiments,
spawning sub-conversations for research tasks, creating throwaway conversations
for one-off queries — the list fills up with entries the user doesn't need to
see day-to-day.

Today, every conversation is visible in `jp conversation ls`. There is no way to
distinguish a conversation the user created interactively from one that was
created programmatically by a tool. A user who sets up sub-agent workflows (see
[RFD 051]) or uses scripts that create temporary conversations quickly ends up
with a noisy, unmanageable list.

Separately, tools that invoke `jp` recursively (e.g. a `jp_query` tool that
delegates work to a sub-agent) need to know which conversation they're running
in so they can create child conversations under the correct parent. The tool
execution context currently provides only the workspace root and the action type
— not the conversation identity.

## Design

### `hidden` flag on `Conversation`

A new boolean field on `Conversation` controls whether it appears in default
listings:

```rust
pub struct Conversation {
    // ... existing fields ...

    /// Whether this conversation is hidden from default listings.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}
```

`hidden` defaults to `false`. It is set at creation time via a new `jp query
--hidden` flag, or toggled after creation with `jp conversation edit --hide` /
`--unhide` (consistent with the existing `edit --local` pattern).

The builder gains a corresponding method:

```rust
impl Conversation {
    #[must_use]
    pub const fn with_hidden(mut self, hidden: bool) -> Self {
        self.hidden = hidden;
        self
    }
}
```

#### Use cases

- **Sub-agent conversations.** A tool that delegates work to a child `jp`
  process creates the sub-conversation with `--hidden` so it doesn't appear in
  the user's default listing.
- **Temporary experiments.** A user forks a conversation to try an alternative
  approach, then hides it when they're done.
- **Scripted workflows.** Automation that creates conversations for batch
  processing or CI tasks hides them from interactive use.

#### `conversation ls` integration

`jp conversation ls` filters out hidden conversations by default. A new
`--hidden` flag includes them:

```sh
jp conversation ls           # non-hidden only (default)
jp conversation ls --hidden  # includes hidden conversations
```

When `--hidden` is passed, the output includes a `Hidden` column (`Y`/`N`) so
the user can distinguish them.

With tree view ([RFD 039]):

```sh
jp conversation ls --tree --hidden
```

Hidden conversations are visually distinguished (dimmed text or `(hidden)`
suffix):

```txt
jp-c17528832001  Refactor error handling in jp_llm          3  5 mins ago
├── jp-c17528842001  [fork] Alternative approach            2  3 mins ago
├── jp-c17528852001  (hidden) Research error types          8  4 mins ago
│   └── jp-c17528862001  (hidden) Deep dive StreamError     5  4 mins ago
└── jp-c17528872001  (hidden) Research retry patterns       6  3 mins ago
```

### `conversation_id` in tool context

The `jp_tool::Context` struct gains a new field:

```rust
pub struct Context {
    pub root: Utf8PathBuf,
    pub action: Action,
    pub conversation_id: ConversationId,
}
```

Tools that need it — such as a `jp_query` tool that creates child conversations
under the current parent — read `conversation_id` from the context. Tools that
don't need it ignore it.

Local tools already receive the `Context` struct as a JSON-serialized template
variable (`{{context}}`). Adding `conversation_id` to `Context` makes it
automatically available to all tools — no new mechanism needed.

## Drawbacks

**Hidden conversations are still on disk.** The `hidden` flag is a UI filter,
not a storage optimization. Users who create many hidden conversations still
accumulate storage. The existing `--tmp=DURATION` flag handles cleanup — hidden
conversations created by automation should typically set an expiration.

## Alternatives

### Tag-based filtering instead of a dedicated `hidden` flag

Use the existing metadata system to tag conversations (e.g. `tags: ["hidden"]`)
and filter based on tags in `conversation ls`.

Rejected because hidden/visible is a binary property that every conversation
has, not an arbitrary classification. A dedicated boolean is simpler to
implement, query, and explain than a tag-based filtering system. Tags may make
sense for other use cases, but this one doesn't need that generality.

### Automatic hiding based on parent relationship

Automatically hide any conversation that is a child of another conversation
(i.e. created via `--fork`).

Rejected because not all child conversations should be hidden. A user who forks
a conversation to explore an alternative approach may want it visible. The
hidden flag should be an explicit choice, not an implicit consequence of the
conversation's position in the tree.

## Non-Goals

- **Conversation archiving or deletion.** This RFD adds a visibility filter, not
  a lifecycle management system. Archiving (moving to cold storage) or bulk
  deletion are separate concerns.

- **Access control.** `hidden` is not a security mechanism. Hidden conversations
  are accessible by ID and visible with `--hidden`. This is a UI convenience,
  not a permission system.

- **Sub-agent workflow design.** How to configure tools that create
  sub-conversations, what configurations to use, and how to structure
  research-plan-implement workflows are covered in [RFD 051].

- **General-purpose tagging.** A tag system for arbitrary conversation
  classification may be valuable, but this RFD addresses only the binary
  hidden/visible distinction. The `hidden` field can migrate to a `"hidden"` tag
  if a tagging system is introduced later.

## Risks and Open Questions

### Interaction with tree view

When a parent conversation is visible but all its children are hidden, should
`conversation ls --tree` show the parent as a leaf, or indicate that hidden
children exist? A `(+3 hidden)` suffix on the parent would hint at the existence
of sub-conversations without revealing them. This is a UX detail that can be
resolved during implementation.

### Garbage collection of hidden conversations

Hidden conversations created by sub-agents may accumulate if `expires_at` is not
set. Should `--hidden` imply a default expiration? This would be convenient for
the sub-agent use case but surprising for users who manually hide a conversation
they want to keep. The safer default is no implicit expiration — sub-agent tool
definitions should set `expires_at` explicitly, or should rely on the existing
semantics in which children are removed when their parent is removed.

## Implementation Plan

### Phase 1: `hidden` flag

Add `hidden: bool` to `Conversation` with the builder method. Add `jp query
--hidden` flag to set it at creation time. Update `conversation ls` to filter
hidden conversations by default and add the `--hidden` flag. Add `conversation
edit --hide` / `--unhide`.

No dependency on other RFDs. Can be merged independently.

### Phase 2: `conversation_id` in tool context

Add `conversation_id: ConversationId` to `jp_tool::Context`. Populate it in the
tool executor. Existing tools are unaffected — tools that don't need it simply
ignore the field in the deserialized `{{context}}` JSON.

No dependency on other RFDs. Can be merged independently.

## References

- [RFD 039: Conversation Trees][RFD 039] — tree-structured conversation
  hierarchy; hidden conversations often appear as children in the tree.
- [RFD 051: Sub-Agent Workflows][RFD 051] — guide describing how to use these
  features to build sub-agent workflows with local tools.

[RFD 039]: 039-conversation-trees.md
[RFD 051]: 051-sub-agent-workflows.md
