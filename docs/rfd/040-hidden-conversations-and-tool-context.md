# RFD 040: Hidden Conversations and Tool Context

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08
- **Required by**: [RFD 051]
- **Summary**: Adds a `hidden` flag to exclude conversations from default
  listings and exposes `conversation_id` in the tool execution context.

## Summary

This RFD adds a `hidden` metadata flag on conversations that excludes them from
default listings, and a `conversation_id` field in the tool execution context.
Together, these let tools, scripts, and sub-agents create conversations
programmatically without cluttering the user's conversation list, and let tools
reference the conversation they run in.

## Motivation

`jp conversation ls` is the primary way users navigate their work, and it shows
every conversation.
When users fork conversations for experiments, spawn sub-conversations for
research tasks ([RFD 051]), or run scripts that create temporary conversations
for one-off queries, the list fills with entries they do not need to see
day-to-day.
Because nothing distinguishes an interactive conversation from one created
programmatically by a tool, these workflows quickly leave the list noisy and
hard to manage.

Separately, tools that invoke `jp` recursively (for example, a `jp_query` tool
that delegates work to a sub-agent) need to know which conversation they are
running in so they can create child conversations under the right parent.
The tool execution context provides only the workspace root and the action type,
not the conversation identity.

## Design

### `hidden` flag on `Conversation`

A `hidden` boolean field on `Conversation` controls whether the conversation
appears in default listings:

```rust
pub struct Conversation {
    // ... existing fields ...

    /// Whether this conversation is hidden from default listings.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}
```

`hidden` defaults to `false`.
Users and tools set it at creation time with `jp query --hidden`, or toggle it
later with `jp conversation edit --hide` and `--unhide` (matching the `edit
--local` pattern).

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
  process creates the sub-conversation with `--hidden` so it stays out of the
  default listing.
- **Temporary experiments.** A user forks a conversation to try an alternative
  approach, then hides it when finished.
- **Scripted workflows.** Automation that creates conversations for batch
  processing or CI tasks hides them from interactive use.

#### `conversation ls` integration

`jp conversation ls` excludes hidden conversations by default.
Passing `--hidden` includes them:

```sh
jp conversation ls           # non-hidden only (default)
jp conversation ls --hidden  # includes hidden conversations
```

With `--hidden`, the output adds a `Hidden` column (`Y`/`N`) to distinguish
hidden entries.

In tree view ([RFD 039]):

```sh
jp conversation ls --tree --hidden
```

Hidden conversations are marked with dimmed text or a `(hidden)` suffix:

```txt
jp-c17528832001  Refactor error handling in jp_llm          3  5 mins ago
├── jp-c17528842001  [fork] Alternative approach            2  3 mins ago
├── jp-c17528852001  (hidden) Research error types          8  4 mins ago
│   └── jp-c17528862001  (hidden) Deep dive StreamError     5  4 mins ago
└── jp-c17528872001  (hidden) Research retry patterns       6  3 mins ago
```

### `conversation_id` in tool context

`jp_tool::Context` gains a `conversation_id` field:

```rust
pub struct Context {
    pub root: Utf8PathBuf,
    pub action: Action,
    pub conversation_id: ConversationId,
}
```

Tools that create child conversations under the current parent, such as a
`jp_query` tool, read `conversation_id` from `Context`; other tools ignore it.
Because local tools already receive `Context` as a JSON-serialized template
variable (`{{context}}`), `conversation_id` is available to all tools without a
new mechanism.

## Drawbacks

**Hidden conversations remain on disk.** The `hidden` flag is a UI filter, not a
storage optimization, so creating many hidden conversations still consumes disk
space.
Automated workflows should pair `--hidden` with `--tmp=DURATION` so temporary
conversations expire.

## Alternatives

### Tag-based filtering

We could use the metadata system to tag conversations (for example, `tags:
["hidden"]`) and filter by tag in `conversation ls`.

Visibility is a binary property of every conversation, not an open-ended
classification.
A dedicated boolean is simpler to implement, query, and explain than general tag
filtering.

### Automatic hiding of child conversations

We could automatically hide any child conversation created via `--fork`.

Not all child conversations should be hidden.
A user who forks a conversation to explore an alternative approach often wants
the fork visible.
Hiding should be an explicit choice rather than a side effect of the
conversation's position in the tree.

## Non-Goals

- **Conversation archiving or deletion.** This RFD adds a visibility filter;
  archiving conversations to cold storage and deleting them in bulk are separate
  lifecycle concerns.

- **Access control.** `hidden` is a UI convenience, not a permission boundary.
  Hidden conversations remain accessible by ID and visible with `--hidden`.

- **Sub-agent workflow design.** [RFD 051] covers how to configure tools that
  create sub-conversations and how to structure research-plan-implement
  workflows.

- **General-purpose tagging.** This RFD addresses only binary visibility.
  If JP introduces a tagging system later, the `hidden` field can migrate to a
  `"hidden"` tag.

## Risks and Open Questions

### Interaction with tree view

When a parent conversation is visible but all its children are hidden, should
`conversation ls --tree` show the parent as a leaf, or indicate that hidden
children exist?
A `(+3 hidden)` suffix on the parent would show that sub-conversations exist
without listing them.
This UX detail can be resolved during implementation.

### Garbage collection of hidden conversations

Hidden conversations created by sub-agents can accumulate if `expires_at` is
unset.
Implying a default expiration from `--hidden` would suit sub-agents, but it
would surprise users who manually hide a conversation they intend to keep.
The safer default is no implicit expiration: sub-agent tool definitions should
set `expires_at` explicitly, or rely on parent removal also removing child
conversations.

## Implementation Plan

### Phase 1: `hidden` flag

Add `hidden: bool` to `Conversation` with its builder method, add `jp query
--hidden` to set it at creation time, add `conversation edit --hide` and
`--unhide`, and update `conversation ls` to exclude hidden conversations by
default unless `--hidden` is passed.
This phase has no dependency on other RFDs and can merge independently.

### Phase 2: `conversation_id` in tool context

Add `conversation_id: ConversationId` to `jp_tool::Context` and populate it in
the tool executor.
Existing tools are unaffected because tools that do not need the field ignore it
in the deserialized `{{context}}` JSON.
This phase has no dependency on other RFDs and can merge independently.

## References

- [RFD 039: Conversation Trees][RFD 039]: Tree-structured conversation
  hierarchy, where hidden conversations often appear as children.
- [RFD 051: Sub-Agent Workflows][RFD 051]: Guide to building sub-agent workflows
  with local tools using these features.

[RFD 039]: 039-conversation-trees.md
[RFD 051]: 051-sub-agent-workflows-with-local-tools.md
