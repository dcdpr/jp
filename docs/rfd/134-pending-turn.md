# RFD 134: Pending Turn

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-08
- **Required by**: [RFD 135]
- **Summary**: Adds a pending turn marker that gathers entries recorded between
  turns, so they belong to the turn they precede.

## Summary

A pending turn is a marker at the end of a conversation stream that precedes
entries recorded after the last finished turn.
When the next turn starts, `start_turn` converts the marker into that turn's
`TurnStart` in place, so between-turn entries belong to the turn they precede
rather than the turn before them.

## Motivation

A conversation stream is a sequence of turns, each opened by a `TurnStart`
event.
Because an entry recorded between two `jp query` invocations has no turn of its
own, it is appended to the tail of the previous turn:

- `jp config set -i <id>` appends a config delta after the last turn's final
  response (`config/set.rs:95`).
- `jp query --cfg …` records its config delta before `start_turn` appends the
  new `TurnStart` (`query.rs:557-563`), placing the new turn's config delta at
  the end of the preceding turn.

Config resolution is unaffected because config deltas apply in stream order and,
as Global entries, survive turn pruning.
However, workspace projection (`assign_turn_indices`) and any external tool that
groups `events.json` by `TurnStart` assign those deltas to the previous turn,
attributing a config change to a turn it never affected.

For provider-visible entries recorded between turns, attaching them to the
previous turn breaks turn-based operations.
Consider a stream `A, X, B`, where `X` is recorded after turn `A` as input for
turn `B`:

- `--fork 1` retains turn `B` and drops `X` along with turn `A`, leaving `B`'s
  response referencing input missing from the fork.
- Compacting turn `A` ([RFD 064]) summarizes `X` into `A` even though `B`
  remains uncompacted.
- Recording `X` after a compaction that already covers the last turn places `X`
  inside the compacted turn range, hiding it from the model.

System notifications queued between turns ([RFD 011]) have this exact lifecycle.
Without a dedicated boundary for between-turn entries, features must either
attach them to the previous turn or use a global scope that ignores turn
position.

## Design

### The marker

```text
[TS ReqA RespA] [P Δ X]               between turns
[TS ReqA RespA] [TS Δ X ReqB RespB]   after `jp query` starts turn B
```

`TS` is a `TurnStart`, `P` is the pending marker, and `Δ` is a config delta.

The marker is `EventKind::PendingTurn(PendingTurn)`, an empty struct serialized
as `{"type": "pending_turn", "timestamp": …}`.
Like `TurnStart`, it is turn-scoped and not provider-visible.

A stream contains at most one pending marker.
Only entries recorded between turns follow it: Global entries (config deltas,
compaction overlays, and patch overlays) and turn-scoped entries added as
pending inputs for the next turn.

### Writing

`ConversationStream::open_pending()` appends a `PendingTurn` marker unless the
stream already ends with a pending section.
Callers that record entries between turns call `open_pending()` before appending
their entries.

`ConversationStream` cannot open the pending marker automatically inside
`add_config_delta`: turns have no end marker, so the stream cannot distinguish
an active turn from the gap between turns.
A config delta written during a running turn (such as an inquiry writing to its
stream copy at `inquiry.rs:291`, or tool config mutation in [RFD 078]) belongs
to the running turn.
`add_config_delta` therefore continues to append directly to the end of the
stream, and between-turn callers explicitly call `open_pending()` first:

- `jp query`, before recording its `--cfg` and editor config deltas.
- `jp config set -i`.
- Plugin `query` delegation (`dispatch.rs:1355`).

### Starting a turn

`start_turn` converts an existing `PendingTurn` marker into a `TurnStart` in
place: the entry retains its event ID and stream position, updates its timestamp
to the current time, and appends the new `ChatRequest` after the pending
entries.
When the stream has no `PendingTurn` marker, `start_turn` appends a new
`TurnStart` at the end of the stream.

### Reading

- `iter_turns`, `iter_events_by_turn`, and `turn_count` end the last turn at the
  `PendingTurn` marker and do not count the pending section as a turn.
- `ConversationStream::pending()` returns the slice of entries following the
  marker.
- Workspace projection numbers pending entries one index past the last turn,
  keeping them outside every compaction range; turn summary generation walks
  `TurnStart` entries and therefore skips the pending section.
- `retain_last_turns` counts `TurnStart` entries and retains every entry after
  the cutoff, so `--fork N` preserves both the last `N` turns and any trailing
  pending entries.
- `retain_turns`, used by `jp conversation fork --first N` and other turn-range
  selectors (`fork.rs:184-185`), associates the pending section with the last
  turn.
  If the range retains the last turn, the pending entries are kept with it.
  If the range drops the last turn, turn-scoped pending entries are removed,
  while the `PendingTurn` marker is retained if any Global entries follow it so
  those config deltas remain attached to the fork's next turn.
  Without that rule, dropping the preceding turn would also drop the
  `PendingTurn` marker and leave its config deltas attached to an earlier turn
  they never affected.
- `sanitize` inserts a `TurnStart` at the start of a stream that has no
  `ChatRequest` (`stream.rs:1168-1191`).
  Recognizing `PendingTurn` as a turn boundary prevents `sanitize` from
  inserting an empty turn ahead of a stream that contains only pending entries.
- `jp conversation print` renders the pending section after the last turn under
  a "pending" heading (instead of a turn number) whenever the turn selection
  includes the last turn, or when the conversation has no turns yet.
  Because `print` renders conversation events rather than config deltas, the
  pending section produces no output until turn-scoped pending events are added.
  Plugin `read_events` returns `PendingTurn` like any other event.

### Replay

`trim_last_turn` removes entries starting from the last turn's first
`ChatRequest` to the end of the stream (rather than starting from its
`TurnStart`), and converts that `TurnStart` back into a `PendingTurn` marker.
Global entries are retained, matching existing behavior.

```text
before:  [TS ReqA RespA] [TS X1 ReqB RespB] [P X2]
replay:  [TS ReqA RespA] [P X1]
then:    [TS ReqA RespA] [TS X1 ReqB' RespB']
```

`X1` was recorded for turn `B` and remains pending for the replayed turn,
whereas `X2` was recorded after turn `B` and is discarded with `B`.
Any stored summary on `B`'s `TurnStart` is cleared when the entry reverts to a
`PendingTurn` marker.

`trim_trailing_empty_turn` follows the same rule: a turn without a `ChatRequest`
reverts to a `PendingTurn` marker, and a `PendingTurn` marker with no entries
after it is removed.

### Retrying an unanswered request

`jp query --no-edit` re-sends the last `ChatRequest` when it is the final event
of its turn, and sends `"continue"` otherwise (`query.rs:929-937`).
Because a failed query followed by `jp config set -i` leaves `[TS ReqB] [P Δ]`
and `PendingTurn` is a turn-scoped entry, two rules change:

- Retry inspection examines the last *started* turn: `last_turn_event` stops at
  any `PendingTurn` marker and returns the last turn-scoped entry before it, so
  `ReqB` is still selected and re-sent.
- Deferred removal deletes the selected request by event ID instead of popping
  the last entry.
  `jp query --no-edit --cfg …` records its config delta (and opens the
  `PendingTurn` marker) after selecting `ReqB` and before removing it; popping
  the tail entry would hit the marker, leave `ReqB` in place, and send `ReqB` a
  second time.

Converting the now-requestless turn back into a `PendingTurn` marker and merging
the trailing `PendingTurn` marker into it preserves the earlier marker position
and keeps all pending entries in stream order:

```text
before:  [TS ReqA RespA] [TS ReqB] [P Δ]
remove:  [TS ReqA RespA] [P Δ]
then:    [TS ReqA RespA] [TS Δ ReqB' RespB']
```

### Repair

A stream containing two `PendingTurn` markers, or a `TurnStart` after a
`PendingTurn` marker, is invalid.
`ConversationStream::from_parts` repairs both shapes on load alongside event ID
normalization ([RFD 097]) rather than failing:

- If a `TurnStart` appears after a `PendingTurn` marker, `from_parts` moves the
  `TurnStart` to the marker's position and removes the marker, preserving the
  turn's event ID and summary while folding the pending entries into that turn.
- Any second `PendingTurn` marker is removed.

The first case occurs when a pre-RFD 134 `jp` binary writes to a stream that has
a `PendingTurn` marker: the older binary preserves `pending_turn` as an unknown
entry (`entry.rs:191-205`) and appends its next `TurnStart` after it.

## Drawbacks

- **In-place entry kind mutation.** Starting a turn mutates a `PendingTurn`
  entry into a `TurnStart` in place.
  Although `ConversationStream` already mutates entries in place
  (`set_turn_summary` edits a `TurnStart` and trim operations remove entries),
  no existing operation changes an entry's kind, so a caller holding the
  marker's event ID observes a `TurnStart` at that ID once the turn starts.
- **Manual marker opening.** Callers that write between turns must call
  `open_pending()` explicitly.
  A between-turn writer that omits the call appends its entries to the previous
  turn without an error.
- **Transient invalid state across binary versions.** Between a write by a
  pre-RFD 134 `jp` binary and the next load by an updated binary, the stream on
  disk holds a `TurnStart` after a `PendingTurn` marker until load-time repair
  normalizes it.
- **Shifted turn numbering for config deltas.** Workspace projection and
  external tools that group `events.json` by `TurnStart` assign between-turn
  config deltas to the turn after them rather than the turn before them.

## Alternatives

**Wrap pending entries in a container entry.** Rejected.
Every consumer (`print`, `jp conversation grep`, plugin `read_events`, config
resolution, and event ID bookkeeping) would have to descend into the container
or silently miss pending entries, and starting a turn would have to unpack the
entries while preserving their event IDs.

**Represent a pending turn as an open `TurnStart` without a `ChatRequest`.**
Rejected.
Every turn-level operation (compaction, summary generation, `start_turn`) would
have to check whether the final `TurnStart` has a request yet, and `--fork 1`
immediately after a between-turn write would keep only the requestless turn.
Using a distinct `PendingTurn` entry makes the unstarted state explicit in the
entry type rather than inferred from missing events.

**Give each between-turn entry its own `TurnStart`.** Rejected.
Requestless turns would inflate turn counts across the codebase, and `--fork 1`
would keep only the trailing between-turn entry without the turn that preceded
it.

**Use Global scope for between-turn inputs.** Rejected.
Global entries preserve stream position, but they survive every turn trim
(`--replay` on turn `B` would retain inputs recorded after `B`) and are skipped
by compaction ranges.

**Leave between-turn entries in the previous turn.** Rejected.
This is the current behavior and causes the fork and compaction bugs described
in [Motivation](#motivation).

## Non-Goals

- **New between-turn event types.** This RFD introduces the `PendingTurn`
  boundary; config deltas are the only existing entries that use it.
- **Mid-turn notification delivery.** Between-turn notifications from [RFD 011]
  can use the pending section once [RFD 011] lands, but delivering notifications
  during a running turn requires its own mechanism.
- **Storing unsent query drafts in the stream.** Drafts such as
  `QUERY_MESSAGE.md` or replies queued during a running turn are user-local
  working state, whereas the conversation stream records committed events.

## Risks and Open Questions

**Preserving the marker's event ID on `TurnStart`.** Retaining the marker's
event ID when converting `PendingTurn` into `TurnStart` makes the marker's ID
become the turn's ID.
Allocating a new ID for the `TurnStart` would invalidate any reference a plugin
held to the marker's ID, so this design preserves the existing ID.

**Overwriting the marker's timestamp.** Converting `PendingTurn` into
`TurnStart` replaces the marker's opening timestamp with the turn's start
timestamp.
Each pending entry retains its own timestamp, so the time at which pending
entries were recorded is still preserved.

**Auditing between-turn writers.** The three writers listed in
[Writing](#writing) cover all current between-turn callers of `add_config_delta`
in `jp_cli`, but any future command that writes to a stream between turns must
also call `open_pending()`.

## Implementation Plan

1. **Marker and stream API.** Add `EventKind::PendingTurn`, its `TYPE_TAGS`
   entry and `encode_event` arm, `open_pending`, `pending`, the `start_turn`
   in-place conversion, turn iteration and counting updates, projection
   numbering, `retain_turns`, and unit tests covering stream shapes.
   This phase is independently mergeable because nothing writes a marker yet.
2. **Trim, retry, and repair.** Update `trim_last_turn` to cut from the first
   `ChatRequest`, `trim_trailing_empty_turn` to revert an empty turn to
   `PendingTurn`, `last_turn_event` to stop at the marker, retry request removal
   to delete by event ID and merge markers, and `from_parts` to repair invalid
   marker ordering on load.
   This phase depends on Phase 1.
3. **Writers and display.** Call `open_pending()` in `jp query`, `jp config set
   -i`, and plugin `query` delegation, and update `jp conversation print` to
   render the pending section when its turn selection includes the last turn.
   This phase depends on Phase 2.
4. **Documentation.** Add a glossary entry for Pending Turn (noting that it has
   no request and is not counted as a turn), and update the Turn glossary entry
   and `TurnStart` doc comment to note that a turn's leading entries can be
   recorded before its `ChatRequest`.
   This phase depends on Phase 3.

## References

- [RFD 011]: System Notification Queue
- [RFD 064]: Non-Destructive Conversation Compaction
- [RFD 078]: Tool Config Mutation
- [RFD 097]: Stable Event Identifiers

[RFD 011]: 011-system-notification-queue.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 097]: 097-stable-event-identifiers.md
[RFD 135]: 135-jp-decide.md
