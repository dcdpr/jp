# Hold a turn's writes briefly so a correction can stop them

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: type=enhancement
- **Parent**: T-1e92tft

A correction sent while a ticket tool is running reaches the turn as the tool's
result (`as_tool_interrupt`), and the write has usually already happened: ticket
writes take milliseconds.
`ticket_create` followed by "actually, not a bug" can leave a duplicate ticket
for review to catch.

A short delay before a writing tool runs in a delegated turn (a few seconds,
configured per tool) would give a correction that arrives in that window the
chance to cancel the call before it writes.

Cheaper than the approval flow in its sibling ticket, and weaker: it only helps
a correction sent within the window, and every write gets slower.
Worth doing only if the approval flow is far off.
