# Let a chat start a new conversation on request

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=plugins
- **Label**: type=enhancement
- **Parent**: T-1e92tft

A chat's conversation rolls over on a fixed schedule (`rollover`: daily, or
weekly on a configured weekday), with no way for participants to start a fresh
conversation early after finishing a long topic.

Because the bridge only inspects inbound messages and the turn's final response,
supporting an on-demand reset requires one of:

- A command prefix handled directly by the bridge before starting a turn, such
  as `/new`.
- A sentinel token in the turn's final response, handled by the bridge like
  `NO_REPLY`.
- A lightweight classifier call in front of the turn that detects reset requests
  before the bridge selects a conversation.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T19:44:39Z

Done with the third option: the decide API (`decide`) is asked one `choice`
question per message, with `answer`, `ignore`, and `new_conversation` as the
options.
`new_conversation` forgets the chat's conversation, so its next turn starts a
new one, and the bridge replies that it will.
Only a message sent to the assistant (direct, a mention, or a reply to it) can
start one, so two people discussing "starting over" don't reset it.
More actions, such as compacting, are one option and one `match` arm each once
the host can do them for a plugin.
