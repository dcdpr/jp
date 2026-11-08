# RFD D73: Pending Turn

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-08
- **Required by**: [RFD D74]
- **Summary**: Adds a pending turn marker that gathers entries recorded between
  turns, so they belong to the turn they precede.

## Summary

A pending turn is a marker at the end of a conversation stream, followed by the
entries recorded since the last turn.
The next turn starts by turning the marker into its `TurnStart`, so those
entries belong to the turn they were recorded for instead of the one before it.

## Motivation

A conversation stream is a sequence of turns, each opened by a `TurnStart`.
An entry recorded between two `jp query` runs has no turn of its own, so it ends
up at the tail of the previous turn:

- `jp config set -i <id>` appends a config delta after the last turn's final
  response (`config/set.rs:95`).
- `jp query --cfg …` records its delta before `start_turn` appends the new
  `TurnStart` (`query.rs:557-563`), so even the delta of the run that starts a
  turn sits in the turn before it.

Config resolution is unaffected: deltas apply in stream order, and as Global
entries they survive pruning.
But projection numbers them with the previous turn (`assign_turn_indices`), and
`jp conversation print` shows them after that turn's answer, so every view that
groups by turn attributes the change to a turn it did not affect.

For an entry the model reads, the previous turn is the wrong owner in every
operation that works on turns.
Take `A, X, B`, where `X` is recorded after turn `A` as input for turn `B`:

- `--fork 1` keeps `B` and drops `X`, so `B`'s answer refers to something the
  fork does not contain.
- A compaction ([RFD 064]) of turn `A` summarises `X` away, while `B` stays
  verbatim.
- An `X` recorded after a compaction that covers the last turn lands inside the
  compacted range, and the model never sees it.

Notifications queued between turns ([RFD 011]) are inputs of this kind.
Without a place for between-turn inputs, every such feature has to choose
between the previous turn and a scope that ignores position.

## Design

### The marker

```text
[TS ReqA RespA] [P Δ X]               between turns
[TS ReqA RespA] [TS Δ X ReqB RespB]   after `jp query` starts turn B
```

`TS` is a `TurnStart`, `P` the pending marker, `Δ` a config delta.

The marker is `EventKind::PendingTurn(PendingTurn)`, an empty struct, stored as
`{"type": "pending_turn", "timestamp": …}`.
Like `TurnStart`, it is turn-scoped and not provider-visible.

A stream has at most one marker.
Only entries recorded between turns follow it: Global entries (config deltas,
compaction and patch overlays), and turn-scoped entries a caller adds as pending
inputs.

### Writing

`ConversationStream::open_pending()` appends a marker unless the stream already
has one.
A caller that records between turns opens it first, then appends.

The stream cannot open it on its own: a turn has no end marker, so the stream
cannot tell whether a turn is still running.
A config delta written during a turn, as an inquiry writes on its copy of the
stream (`inquiry.rs:291`) or as tool config mutation ([RFD 078]) will, belongs
to that turn.
So `add_config_delta` keeps appending at the end, and the between-turn writers
open the marker before calling it:

- `jp query`, before recording its `--cfg` and editor deltas.
- `jp config set -i`.
- Plugin `query` delegation (`dispatch.rs:1355`).

### Starting a turn

`start_turn` turns the marker into a `TurnStart` in place: the entry keeps its
event ID and position, takes the current time, and the request is appended after
the pending entries.
Without a marker, `start_turn` appends a `TurnStart` as it does today.

### Reading

- `iter_turns`, `iter_events_by_turn`, and `turn_count` end the last turn at the
  marker, and do not count the pending entries as a turn.
- `ConversationStream::pending()` returns the entries after the marker.
- Projection numbers pending entries one past the last turn, so no compaction
  range reaches them, and turn summary generation, which walks `TurnStart`s,
  does not see them.
- `retain_last_turns` counts `TurnStart`s and keeps everything after its cut, so
  `--fork N` keeps the last N turns and the pending entries without a change.
- `sanitize` inserts a `TurnStart` at the start of a stream that has no
  `ChatRequest` (`stream.rs:1168-1191`).
  It treats the marker as a turn boundary instead, so a stream holding only
  pending entries gets no empty turn ahead of them.
- `jp conversation print` shows pending entries after the last turn, under a
  "pending" heading instead of a turn number.
  Plugin `read_events` returns the marker like any other event.

### Replay

`trim_last_turn` removes from the last turn's first `ChatRequest` to the end of
the stream, instead of from its `TurnStart`, and turns that `TurnStart` back
into a marker.
Global entries stay, as they do today.

```text
before:  [TS ReqA RespA] [TS X1 ReqB RespB] [P X2]
replay:  [TS ReqA RespA] [P X1]
then:    [TS ReqA RespA] [TS X1 ReqB' RespB']
```

`X1` was recorded for `B` and stays; `X2` was recorded after `B` and goes with
it.
`B`'s stored summary goes with its `TurnStart`.
With config deltas as the only between-turn entries today, replay behaves as it
does now.

`trim_trailing_empty_turn` changes with it: a turn left without a `ChatRequest`
(after `--no-edit` pops a request) becomes a marker again, and a marker with
nothing after it is removed.

### Repair

A stream with two markers, or with a `TurnStart` after its marker, is invalid.
`ConversationStream::from_parts` repairs it on load, as it settles event IDs
([RFD 097]), rather than failing:

- A `TurnStart` after the marker moves to the marker's position and the marker
  is removed.
  The turn keeps its ID and summary, and gains the pending entries.
- A second marker is removed.

The first case is what a `jp` from before this RFD produces.
It reads `pending_turn` as an unknown entry (`entry.rs:191-205`), which it keeps
verbatim, and appends its next turn after it.

## Drawbacks

- An entry changes type when a turn starts.
  The stream is not append-only today (`set_turn_summary` edits a `TurnStart`,
  and trims remove entries), but nothing changes an entry's kind.
  A reader that held the marker's event ID finds a `TurnStart` under it
  afterwards.
- Opening the marker is each caller's job.
  A between-turn writer that forgets puts its entries in the previous turn, as
  today, and nothing catches it.
- Between a write by an older `jp` and the next load by a newer one, the stream
  holds an invalid shape that only the newer binary repairs.
- Config deltas move to a different turn in `print`, in projection's turn
  numbering, and in any tool that reads `events.json` and groups by turn.

## Alternatives

**A container entry that holds the pending entries.** Every reader would have to
look inside it: `print`, `jp conversation grep`, plugin `read_events`, config
resolution, and event ID bookkeeping, and each that does not would skip pending
entries without an error.
Starting a turn would move entries out of it and have to keep their IDs.

**An open turn: a `TurnStart` without a request, which the next request joins.**
Every operation that writes or finishes turns has to check whether the last turn
has a request yet: compaction, summary generation, `start_turn`.
`--fork 1` right after a between-turn entry keeps only the open turn.
The marker makes "not started" an entry type instead of a property inferred from
what a turn lacks.

**Each between-turn entry its own turn.** Needs no marker, but turns without a
request appear everywhere turns are counted, and `--fork 1` keeps the entry
without the turn it was recorded after.

**Global scope for between-turn inputs.** Keeps position, but survives every
trim: `--replay` of `B` would keep inputs recorded after `B`, and compaction
ranges skip them.

**Leave them in the previous turn.** Today's behaviour, with the three failures
in the Motivation.

## Non-Goals

- **New between-turn entry types.** This RFD adds the place; config deltas are
  the only entries that use it.
- **Notification delivery.** [RFD 011]'s between-turn notifications fit here if
  that RFD lands; delivery during a turn needs its own mechanism.
- **Unsent text,** such as the `QUERY_MESSAGE.md` draft or a reply queued during
  a turn.
  It is local to one user and still being edited, and the stream holds what was
  sent.

## Risks and Open Questions

**The marker's event ID.** Keeping it makes the marker's ID the turn's ID.
A fresh ID for the `TurnStart` would leave a plugin that held the marker's ID
with a dangling reference instead.
This draft keeps the ID.

**The marker's timestamp.** The `TurnStart` takes the time the turn started, so
the time the marker was opened is lost.
Pending entries keep their own timestamps, which hold the same information.

**Finding every between-turn writer.** The three writers above come from the
callers of `add_config_delta` in `jp_cli`.
A new command that records between turns has to know to open the marker.

## Implementation Plan

1. **Marker and stream API.** `EventKind::PendingTurn`, its `TYPE_TAGS` entry
   and `encode_event` arm, `open_pending`, `pending`, the `start_turn` change,
   turn iteration and counting, and projection numbering.
   Unit tests over stream shapes.
   Merges alone: nothing writes a marker yet.
2. **Trim and repair.** `trim_last_turn` from the first request,
   `trim_trailing_empty_turn` back to pending, and load-time repair.
   Depends on 1.
3. **Writers and display.** `jp query`, `jp config set -i`, and plugin `query`
   delegation open the marker; `print` shows the pending section.
   Depends on 2.
4. **Documentation.** A glossary entry for Pending Turn (not a turn: no request,
   not counted), and the Turn entry and `TurnStart` doc gain that a turn's
   leading entries can be recorded before its request.
   Depends on 3.

## References

- [RFD 011]: System Notification Queue
- [RFD 064]: Non-Destructive Conversation Compaction
- [RFD 078]: Tool Config Mutation
- [RFD 097]: Stable Event Identifiers

[RFD 011]: ../011-system-notification-queue.md
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 078]: ../078-tool-config-mutation.md
[RFD 097]: ../097-stable-event-identifiers.md
[RFD D74]: D74-jp-decide.md
