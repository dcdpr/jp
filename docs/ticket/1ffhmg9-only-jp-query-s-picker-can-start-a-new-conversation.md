# Only jp query's picker can start a new conversation

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=enhancement

`Commands::allows_new_from_picker` (`crates/jp_cli/src/cmd.rs:287`) returns
`false` for every command except `jp query`, so only `jp query`'s picker offers
an option to start a new conversation.
Conversation creation is also implemented inside `jp query`: the picker returns
`start_new`, and `Query::acquire_lock` creates the conversation
(`crates/jp_cli/src/cmd/query.rs:1238`).

Other commands also write to the conversation selected in the picker, making a
new conversation a valid target for them as well:

- `jp config set -i '?'` records a config delta in the selected conversation.
- `jp decide -i '?'` (proposed in RFD 135) records a decision in the selected
  conversation.

Open questions:

- **Which commands should allow creating a conversation from the picker.**
  Commands that write to the conversation are the natural candidates, whereas
  read-only commands (`print`, `grep`) have nothing to display for a newly
  created conversation.
- **Whether conversation creation should move into shared target resolution**
  (`crates/jp_cli/src/cmd/target.rs`) so every qualifying command initializes
  the conversation with the same configuration, title behavior, and session
  activation.
