# jp config set reports success for a clear it never records

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-16
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=bug

`jp config set --id <id> --cfg assistant.name:=null` prints `Set configuration
in conversation <id>` and records nothing.
The same argument through `jp query` records a delta whose `unsets` holds
`assistant.name`, and the option stays cleared for the rest of the conversation.

## Why

`Set::run` builds its payload with
`config_pipeline::build_partial_from_cfg_args`
(`crates/jp_cli/src/cmd/config/set.rs:28`), which applies the arguments onto
`PartialAppConfig::empty()` (`config_pipeline.rs:578`).
A `:=null` assignment clears a field, so clearing a field of an already-empty
partial produces an empty partial: the payload carries no trace of what the user
asked to clear.

`override_to_record` then merges that empty partial onto the conversation's
accumulated state, resolves both sides, finds them equal, and returns `None`
(`config_pipeline.rs:64-78`), so no `ConfigDelta` is appended.
The success line is printed unconditionally afterwards (`set.rs:68-69`).

The file target has the same hole: `set_in_file` merges the empty partial into
the config file and writes it back unchanged (`set.rs:85-90`).

A partial cannot express "cleared" — that is what `ApplyDelta::unsets` exists
for ([RFD 070]).
The query path reaches it through `turn_config_delta`/`delta_with_unsets`
(`cmd/query.rs:2333-2351`); the `config set` path has no equivalent.

## Fix

Carry the cleared paths alongside the partial, the way the conversation layer
now does: have `build_partial_from_cfg_args` report the paths its arguments
cleared, and have `set_in_conversations` pass them to `ApplyDelta::with_unsets`
so a clear becomes a recorded delta.
For the file target, a clear means removing the key from the file, which
`ConfigFile`'s format-preserving edit already knows how to express.

Failing either, the command should refuse `:=null` with an error naming the
working alternative, rather than reporting a change it did not make.

## Verifying

A test asserting that `jp config set --id <id> --cfg assistant.name:=null`
leaves the conversation resolving `assistant.name` to `None` on the next
invocation, mirroring
`resolve_config_keeps_a_cleared_conversation_field_across_invocations` in
`crates/jp_cli/src/cmd/query_tests.rs`.

[RFD 070]: https://jp.computer/rfd/070
