# Scaffold jp-serve-chat with a transport-neutral turn loop

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=feature
- **Parent**: T-1e92tft

Create the `jp-serve-chat` command plugin (`jp serve chat`) under
`crates/plugins/command/serve-chat`.
Like `jp-serve-web`, the plugin does not call models or execute tools itself;
the host runs every turn.
This ticket covers the transport-neutral turn loop and fake-transport test
suite.

```toml
[plugins.command.serve-chat.options]
transport = "signal"
persona = "personas/planner"
admin = "maintainer"

[plugins.command.serve-chat.options.people.maintainer]
name = "<name>"
signal = "<account id>"

[plugins.command.serve-chat.options.people.client]
name = "<name>"
signal = "<account id>"
```

- **Transport.** Define a transport interface for inbound messages (chat, sender
  IDs, text, attachment count), outbound replies, and typing indicators.
  Replies always go back to the originating chat; the model cannot choose a
  recipient.

- **People.** Drop messages from unconfigured senders and log the sender ID
  without the message body.
  Pass the sender's display name on each query via `--cfg` as `user.name` and as
  `<Name> via jp` for the `ticket`, `rfd`, and `wiki` plugin `assistant` options
  alongside the configured `persona`.

- **Conversations.** Route each chat to one conversation per rollover period,
  created with `new = true`, a title naming the chat and period date, and
  `expires_in`.
  Persist the chat-to-conversation mapping across restarts.

- **One turn at a time.** Forward messages arriving during an active turn on the
  same chat as `interrupt` requests (`action = "reply"`).
  If the host rejects the interrupt because the turn already finished, queue the
  message for the next `query`.
  Batch messages queued between turns into a single `query`.

- **Answers.** When `query_complete` arrives, fetch the conversation events via
  `read_events` and send the assistant's final response to the chat.
  On turn failure, send a brief error notice to the chat and log the full error.

- Reply to messages with attachments explaining that the bridge only reads text,
  rather than dropping attachments silently.

- [x] Tests through a fake transport and a scripted host: allow-list, daily
      rollover, interrupt delivered, interrupt refused and resent as a query,
      queued messages batched, the `cfg` each query carries
