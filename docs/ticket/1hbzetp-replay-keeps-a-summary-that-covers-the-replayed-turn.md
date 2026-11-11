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
keeps every Global entry, compaction overlays included (`:310-314`), without
checking an overlay's range.

Compact a range that includes the last turn, then run `jp query --replay`:

```text
before:  [TS ReqA RespA] [TS ReqB RespB] [C summary covering turn 1]
replay:  [TS ReqA RespA] [C summary covering turn 1] [TS ReqB' ...]
```

The replacement turn takes index 1, which the kept overlay still covers.
Projection replaces every event in a summary-covered range, requests included
(`crates/jp_conversation/src/stream/projection.rs:207-257`), so the model
answers the old summary instead of the replayed request, and nothing reports it.

`ConversationStream::retain` already applies the right rule
(`stream.rs:914-921`): an overlay whose `to_turn` reaches the earliest removed
turn is dropped, and one that ends before it is kept, since positional anchors
cannot be rebased (RFD 097).
`trim_last_turn` should follow it, keeping config deltas and patch overlays as
today.

RFD 134 relies on this fix: its pending section must stay outside every
compaction range after a replay.

- [ ] `trim_last_turn` drops compaction overlays reaching the trimmed turn.
- [ ] A regression test: a summary covering the last turn, a replay, and
      projection that shows the replayed request rather than the summary.
