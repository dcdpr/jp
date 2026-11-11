# Scaffold jp-serve-chat with a transport-neutral turn loop

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=feature
- **Parent**: T-1e92tft

A command plugin at `crates/plugins/command/serve-chat`, binary `jp-serve-chat`,
command `jp serve chat`, built like `jp-serve-web`: it runs no model and no
tool, and every turn it starts the host runs.
This ticket is the plugin without a real transport, driven through a fake one in
tests.

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

- **Transport.** A trait over inbound messages (chat, sender, text, whether it
  carried attachments), sending text to a chat, and a typing indicator.
  Replies always go to the chat the message came from; the model never picks a
  recipient.

- **People.** A message from a sender who isn't configured is dropped, and
  logged without its text.
  A person's name reaches the turn as `user.name`, and as
  `plugins.command.ticket.options.assistant` and
  `plugins.command.wiki.options.assistant` ("<name> via jp"), through the
  query's `cfg`, beside the persona.

- **Conversations.** One per chat per day, created with `new`, a title naming
  the chat and the date, and an `expires_in`.
  The mapping survives a restart.

- **One turn at a time.** A message arriving during a turn goes out as
  `interrupt` with `action = "reply"` and an `id`; on an `error` answer (the
  turn had ended) it is sent as the next `query`.
  Messages waiting between turns go out together as one `query`.

- **Answers.** After `query_complete`, read the turn's events with `read_events`
  and send the assistant's text.
  A failed turn sends a short message to the chat; the detail goes to the log.

- An attachment gets a reply that the bot reads text only, rather than being
  dropped silently.

- [x] Tests through a fake transport and a scripted host: allow-list, daily
      rollover, interrupt delivered, interrupt refused and resent as a query,
      queued messages batched, the `cfg` each query carries
