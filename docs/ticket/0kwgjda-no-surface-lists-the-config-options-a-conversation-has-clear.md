# No surface lists the config options a conversation has cleared

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-16
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=enhancement

A conversation can clear a config option (`jp query --cfg
assistant.name:=null`), and the clear holds for the rest of the conversation:
the file layer cannot restore the option, because the pipeline reapplies the
stream's cleared paths after filling from the base.

Nothing reports which paths a conversation currently holds clear.

`jp config show` resolves the file, env, and `--cfg` layers only —
`Commands::Config(_)` returns an empty conversation layer
(`crates/jp_cli/src/cmd.rs:200-207`), and `cmd/config/show.rs` never touches a
conversation.
So the option reads as absent, with no indication that a conversation-level
clear is what holds it there.
The only trace is the `unsets` array inside the stored event stream, reachable
through `jp conversation edit --events`.

The named input: a user clears `assistant.name` in a conversation, sets
`assistant.name` in `.jp/config.toml` a week later, and has no way to find out
why that conversation ignores it.
`RUST_LOG=debug` logs the suppressed paths at the moment they apply, which helps
a user who already suspects the cause, but no read surface answers the question
directly.

## Shape

Two candidates, not exclusive:

- Teach `jp config show` to apply the conversation layer when given `--id`, and
  mark cleared paths distinctly from unset ones.
- Report them in `jp conversation show`, alongside the conversation's other
  stored state.

`ConversationStream::config_unsets()` already computes this list, so either
surface is a read away.

Related: [RFD 060] (Config Explain) would subsume this if it lands first — a
per-field provenance display answers "what is holding this field empty?" as a
special case.

[RFD 060]: https://jp.computer/rfd/060
