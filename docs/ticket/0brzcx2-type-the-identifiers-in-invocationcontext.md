# Type the identifiers in InvocationContext

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-01

`jp_llm::tool::InvocationContext` carries both identifiers as bare strings:

```rust
pub struct InvocationContext {
    pub workspace_id: String,
    pub conversation_id: String,
}
```

Both have domain types.
Passing them as strings means every construction site formats them by hand, and
nothing stops the two fields being filled in the wrong order.

The two halves are not equally cheap.

`conversation_id` is straightforward: `jp_llm` already depends on
`jp_conversation`, so `ConversationId` is available today.

`workspace_id` is not.
`jp_llm` does not depend on `jp_workspace`, and adding that edge points a
lower-level crate at an app-level one.
The alternative is moving `jp_workspace::Id` into the existing `jp_id` crate so
both can depend on it, which is a decision of its own.

Two things to check before starting:

- `InvocationContext::default()` is used around sixty times in
  `crates/jp_cli/src/cmd/query/turn_loop_tests.rs`, so whatever the fields
  become has to keep working with `Default`.
- `crates/jp_cli/src/render/tool.rs` serialises both fields straight into the
  template context a local tool sees (`context.workspace_id`,
  `context.conversation_id`).
  That rendered contract is string-shaped and must not change.

There are 177 references to the type across the tree, so this is mechanical but
wide.

Raised in review of PR \#962, where `TurnInputs` holds a typed
`jp_workspace::Id` and converts to a string at this boundary.
