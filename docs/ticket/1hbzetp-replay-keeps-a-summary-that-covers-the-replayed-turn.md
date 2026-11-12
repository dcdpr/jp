# Replay keeps a summary that covers the replayed turn

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=conversation
- **Label**: package=jp_conversation
- **Label**: relevant-rfd=134
- **Label**: type=bug

`ConversationStream::trim_last_turn`
(`crates/jp_conversation/src/stream.rs:289`) removes the last turn's entries but
retains every Global entry, including compaction overlays (`:310-314`), without
checking the overlay's turn range.

If a user compacts a range that includes the last turn and then runs `jp query
--replay`:

```text
before:  [TS ReqA RespA] [TS ReqB RespB] [C summary covering turn 1]
replay:  [TS ReqA RespA] [C summary covering turn 1] [TS ReqB' ...]
```

The replacement turn receives index `1`, which the retained compaction overlay
still covers.
Because projection replaces every event in a summary-covered range, including
`ChatRequest` events
(`crates/jp_conversation/src/stream/projection.rs:207-257`), the model silently
receives the old compaction summary instead of the replayed request.

`ConversationStream::retain` already enforces the right rule
(`stream.rs:914-921`): any compaction overlay whose `to_turn` reaches the
earliest removed turn is dropped, while overlays ending before it are kept
because positional turn anchors cannot be rebased (RFD 097).
`trim_last_turn` should apply the same rule while continuing to retain config
deltas and patch overlays.

RFD 134 relies on this fix so that its pending section remains outside every
compaction range after a replay.

- [ ] Drop compaction overlays whose range reaches the trimmed turn in
      `trim_last_turn`.
- [ ] Add a regression test verifying that replaying a turn covered by a
      compaction summary drops the summary and projects the replayed request.
