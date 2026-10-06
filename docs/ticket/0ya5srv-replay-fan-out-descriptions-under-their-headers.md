# Replay fan-out descriptions under their headers

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: client=cli
- **Label**: package=jp_cli

A fanned-out call whose tool has a custom `style.parameters` formatter shows
each operation's header followed by its description when it runs.
`jp conversation print` shows every operation's header first and then all the
descriptions as one block (`render/turn.rs`), because the descriptions are
stored as one newline-joined string in the `ToolCallRequest` event's metadata
(RFD 117, "Rendering and replay").

Replay then no longer says which description belongs to which operation.
The live output is correct, and nothing is lost: the text is all there, in
operation order.

## Proposal

Store the descriptions as a list, one per operation, and have replay print each
under its header.
Reading must still accept the single string conversations already have on disk,
rendering it as it does today.

Raised in review of #1182 (comment 4194442765).
