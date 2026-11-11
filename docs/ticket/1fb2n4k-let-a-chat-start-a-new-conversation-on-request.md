# Let a chat start a new conversation on request

- **Status**: Todo
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
