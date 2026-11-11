# Let a chat start a new conversation on request

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=plugins
- **Label**: type=enhancement
- **Parent**: T-1e92tft

A chat's conversation rolls over on a schedule (`rollover`: daily, or weekly on
a given day), with no way for the people in it to start fresh sooner, for
example after a long thread on one topic.

Asking the assistant for it ("start a new conversation") would need the turn to
tell the bridge, which today only reads the turn's answer.
Options, cheapest first:

- A word the bridge recognizes before any turn runs, such as `/new`, handled in
  code.
- An answer the turn ends with, recognized by the bridge the way `NO_REPLY` is.
- A small, cheap model in front of the turn that classifies a message as "start
  a new conversation" or "for the assistant", so the reset is decided
  deterministically in the bridge rather than by the persona.
