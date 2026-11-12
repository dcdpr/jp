# Start a temporary conversation when none is active

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=pre-rfd

When a session has no active conversation, `jp query` falls back to
`conversation.default_id`, then to the interactive conversation picker, and
errors if the picker cannot prompt (`resolve_from_session_or_picker`,
`crates/jp_cli/src/cmd/target.rs:735`).
`jp decide` (proposed in RFD 135) resolves its conversation in the same order,
but runs without a conversation where `jp query` would open the picker.

Proposal for both commands: where resolution would fall back to the picker,
create a temporary conversation (equivalent to `jp query --new --tmp`) and set
it as the session's active conversation.
A bare `--tmp` conversation persists only while it remains the active
conversation (`crates/jp_cli/src/cmd/query/args.rs:610-631`) and can be promoted
to a permanent conversation with `jp c edit --no-tmp`.
Starting a temporary conversation automatically lets a `jp decide` run in a
fresh terminal record its decision into the same conversation that the next `jp
query` uses.

Both commands should adopt this behavior together so they continue to share the
same default conversation resolution.

Open questions:

- **Behavior when no session identity is available.** Without a session to keep
  a temporary conversation active, `jp decide` can fall back to running without
  a conversation, whereas `jp query` currently errors.
- **How users opt back into the picker.** `jp decide` has `decision.conversation
  = "required"`, whereas `jp query` has no equivalent setting and already uses
  `conversation.default_id = "ask"` as its default.
- **Whether non-interactive `jp query` runs should also create a temporary
  conversation** instead of failing when no conversation is active.
- **Whether `--tmp` should still require `--new`** once temporary conversations
  can be created implicitly.
- **Conversation title generation.** A conversation created by `jp decide`
  starts with a pending `Decision` event and no turns, whereas `jp query`
  currently generates a title only when it creates a conversation, forks one, or
  finds an empty event stream (`crates/jp_cli/src/cmd/query.rs:604`).
  `jp query` would also need to generate a title when starting the first turn of
  a conversation that contains only pending entries.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T15:20:36Z

RFD 134 resolves the title generation question for pending-only conversations: a
stream containing only pending entries has `turn_count() == 0` and is treated as
a new conversation when deciding whether to generate a title on the first `jp
query`.
A temporary conversation created by `jp decide` under this ticket will get its
title through the same rule.
