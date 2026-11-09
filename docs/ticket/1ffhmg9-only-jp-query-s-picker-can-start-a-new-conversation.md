# Only jp query's picker can start a new conversation

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=enhancement

`Commands::allows_new_from_picker` (`crates/jp_cli/src/cmd.rs:287`) returns
`false` for every command except `jp query`, so only its picker lists "start a
new conversation".
Creating the conversation is also `jp query`'s own code: the picker's choice
comes back as `start_new`, and `Query::acquire_lock` creates the conversation
(`crates/jp_cli/src/cmd/query.rs:1238`).

Other commands write into the conversation they pick, and a new conversation is
a valid target for them too:

- `jp config set -i '?'` records a config delta in the chosen conversation.
- `jp decide -i '?'`, proposed in RFD 135, records a decision in it.

Open questions:

- Which commands qualify.
  Writing into the conversation looks like the rule; `print`, `grep`, and other
  read-only commands have nothing to show for an empty one.
- Whether creation moves out of `jp query` into the shared target resolution in
  `crates/jp_cli/src/cmd/target.rs`, so every qualifying command creates it the
  same way, with the same config, title, and session activation.
