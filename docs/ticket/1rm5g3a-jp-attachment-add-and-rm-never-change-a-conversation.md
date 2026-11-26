# jp attachment add and rm never change a conversation

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=bug

`jp attachment add`, `jp attachment rm`, and the `jp aa` shorthand exit
successfully and change nothing.

They load no conversation (`Commands::conversation_load_request` returns
`ConversationLoadRequest::none()` for them in `crates/jp_cli/src/cmd.rs`), and
their `run` methods are empty.
Their `apply_cli_config` edits the in-memory config of the invocation only, so
the edit is dropped when the command exits.
`jp attachment ls` reads the same config, so it lists the workspace's
attachments rather than the active conversation's.

The commands are still recommended in places a user acts on:

- The `mcp` handler's error for a retired attachment tells the user to remove it
  with `jp attachment rm` (`UnsupportedResolution` in
  `crates/jp_attachment_mcp_resources/src/lib.rs`), which leaves the attachment
  in place, so every query keeps failing.
- The `--help` text of `jp attachment add` and `rm` says they add to and remove
  from the context.

Until the commands write to the conversation, the working way to change a stored
attachment list is `jp config set --id <id> --cfg @file`, with a file that
declares `[conversation.attachments]` with `strategy = "replace"` and lists
every attachment to keep.
A `--cfg conversation.attachments=...` key assignment appends instead.
`config_set_with_a_replace_file_replaces_a_conversations_attachments` in
`crates/jp_cli/src/config_pipeline_tests.rs` pins that path.

- [x] `jp attachment add` and `rm` record their change on the targeted
      conversation, the way `jp config set --id` does
- [x] `jp attachment ls` lists the targeted conversation's attachments
- [x] The `mcp` handler's removal hint works as written

## Comments

-----

- **From**: jp
- **Date**: 2026-11-25T09:42:52Z

Fixed in the working tree, not yet committed.

- `add`, `rm`, `ls` (and `jp aa`) take `--id`, defaulting to the session's
  active conversation.
- `add` records an appending delta with only the attachments the conversation
  lacks; an attachment it already has is reported on stderr and records nothing.
- `rm` records the remaining list with `strategy = "replace"`.
  This removes attachments a config file declared too, because the conversation
  holds its own resolved copy and `partial_with_conversation` fills lists from
  the file layer only when the conversation has none (`MergeableVec::fill_from`
  keeps an empty list).
  `rm_of_the_last_attachment_is_not_refilled_from_the_file_layer` pins that
  through the pipeline.
  An attachment the conversation doesn't have is reported on stderr.
- `ls` loads the conversation's config layer (`Commands::Attachment` routes
  through `query::conversation_config`).
- The `mcp` hint prints the URL `to_url()` produces for the stored attachment,
  which is what `rm` compares against.
  Checked by reading the code, not run end-to-end.

The URL match is exact, by `Url` equality: `%20` and `+` for a space don't match
each other.
`jp c show` prints the form `rm` expects.

-----

- **From**: jp
- **Date**: 2026-11-25T14:47:49Z

Corrections to the comment above, after review on PR #1389.

The fix is committed (`a81b9a81e`), and a follow-up commit on the same PR
changes three things:

- `ls` reads the stored attachment list from the conversation's events, as `jp
  conversation show` does, rather than loading the conversation's config layer.
  `--cfg` overrides and `--cfg WORKSPACE` no longer change what it lists, and
  `Commands::Attachment` no longer routes through `query::conversation_config`.
- `rm` resolves a file path without requiring the file to exist
  (`AttachmentUrlOrPath::parse_unchecked`), so an attachment whose file was
  deleted or renamed is still removed by its path.
- `add` and `jp query -a` refuse a retired MCP resource URI: the `mcp` handler's
  `Handler::canonical` returns an error for it.
  A conversation already holding one still removes it with `rm`.
  A URI that arrives through config is not checked; that is [T-1rr0xfp].

[T-1rr0xfp]: https://jp.computer/T-1rr0xfp
