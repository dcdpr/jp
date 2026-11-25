# Validate attachments that reach a conversation from config

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: client=cli
- **Label**: package=jp_attachment
- **Label**: package=jp_cli
- **Label**: relevant-rfd=065
- **Label**: type=follow-up

`jp attachment add` and `jp query -a` run each new attachment through
`canonical_attachment`, which asks the scheme's handler (`Handler::canonical` in
`jp_attachment`) for the URI's canonical form and fails if the handler rejects
it.
That is how a retired `mcp+…` URI is refused before it is stored.

An attachment that arrives through config is not checked: one declared in a
config file, or set with `jp config set --id <id> --cfg
conversation.attachments=…`.
A retired MCP URI declared in a knowledge file is copied into every new
conversation, and every query on it fails with `UnsupportedResolution` until it
is removed.
`jp attachment rm` clears it from one conversation, and the next new
conversation copies it again.

## Where the check can and cannot go

- Not on the resolved config.
  Every command resolves one, so a conversation already holding the URI would
  fail every command, including the `jp attachment rm` the error tells the user
  to run.
- Not in `jp_config` or `jp_conversation`.
  `Validator` checks pure cross-field rules, and neither crate knows about
  attachment handlers; only `jp_cli` links them all.
- At the points an attachment is first written to a conversation, checking only
  the attachments the conversation gains (present after the write, absent
  before).
  That keeps a conversation that already holds one recoverable.

Those points are `Query::create_new_conversation` (the first snapshot), `jp
config set --id`, the query's `--cfg` delta, and plugin dispatch.
Four call sites that each have to remember a helper is brittle: a fifth writer
that forgets it silently reverts to the current behavior.
This fits better as part of a redesign that gives attachments one write path,
such as RFD 065's typed resource model, than as a helper threaded through the
existing writers.

Once config is merged it no longer records which file declared an attachment, so
the refusal can name the URI but not the file.
A per-file warning in `load_partial_configs_from_files` could name it, but would
print on every command until the file is fixed.

- [ ] A retired MCP URI declared in a config file is refused when a conversation
      is created
- [ ] A retired MCP URI set with `jp config set --id` is refused
- [ ] A conversation already holding one still loads and can remove it
