# RFD 098: Request-Response Event Linking

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-07
- **Extends**: [RFD 097]
- **Summary**: Links answering events to their originating `ChatRequest` via a
  persistent `request_id` field so answer scope survives manual edits.

## Summary

This RFD adds an optional `request_id` field to the stream-entry wrapper that
links each answering event (`ChatResponse`, `ToolCallRequest`,
`ToolCallResponse`, `InquiryRequest`, and `InquiryResponse`) to the `event_id`
([RFD 097]) of its originating `ChatRequest`.
Stamping `request_id` at insertion time makes request-response scope explicit on
disk so structural edits to `events.json` produce detectable orphan or
reordering states rather than silent positional aliasing.

## Terminology

- **Originating request**: the `ChatRequest` being answered.
- **Immediate request**: the paired `ToolCallRequest` or `InquiryRequest` that a
  `ToolCallResponse` or `InquiryResponse` matches via its payload `id` field.
- **Request link**: the `request_id` reference from an answering event to its
  originating request.

The request link is separate from immediate-request pairing:
`ToolCallResponse.id` and `InquiryResponse.id` keep their existing semantics.

## Motivation

A conversation stream interleaves `ChatRequest` events from the user with the
events that answer them: chat responses, tool calls, and inquiries.
The binding between an answering event and its originating request is currently
positional: a reader assumes the event belongs to the most recent request in the
same turn.

Positional binding is fragile when editing `events.json` by hand.
Deleting a digression, rewinding an in-flight query, or dropping a noisy tool
call can silently change which request a response appears to answer.
Positional binding is also ambiguous without edits: interrupt replies can inject
additional `ChatRequest` events mid-turn, so a turn can contain more than one
request.
Proposed capabilities such as branching, undo, compaction anchoring, faithful
turn reconstruction, and multi-participant conversations need to know
unambiguously which request each event answers.

RFD 097 gives every stream entry a stable `event_id` and leaves reference
semantics to its consumers, requiring each reference-bearing feature to define
its own orphan and ambiguity handling.
This RFD records the answer-to-request relationship as an explicit `event_id`
reference and defines that handling.

## Design

### Linked event families

Each event produced while answering a `ChatRequest` carries a request link to
that `ChatRequest`:

- `ChatResponse` (message, reasoning, and structured variants)
- `ToolCallRequest`
- `ToolCallResponse`
- `InquiryRequest`
- `InquiryResponse`

The remaining event kinds do not carry a request link:

- `TurnStart` and `ChatRequest`: open a scope rather than answering one.
- `ConfigDelta` and `Compaction`: apply to the stream rather than to a single
  answer.
- Unknown (forward-compatibility) events: opaque entries passed through
  verbatim.

All five answering families link to the originating `ChatRequest`, not to their
immediate request.
Tool and inquiry events keep their immediate pairing through the payload `id`,
while `request_id` groups the entire answer (responses, tool round-trips, and
inquiries) under the request that triggered it.
Linking all five families keeps that grouping in one place so consumers do not
have to reconstruct it.

### Where the link lives

`request_id` is a wrapper-level field alongside `event_id` on the stream-entry
wrapper defined by RFD 097:

```rust
struct InternalEvent {
    event_id: EventId,
    request_id: Option<EventId>,
    payload: EventPayload,
}
```

Putting `request_id` on the wrapper rather than the payload follows the same
rationale as `event_id` in RFD 097:

- **Constructors stay unchanged.** `ChatResponse`, `ToolCallRequest`, and the
  other payload types gain no field or constructor argument.
  `EventBuilder` in `jp_llm` and the turn coordinator in `jp_cli` construct
  payloads without stream context and need no plumbing to learn the request ID.
- **The stream owns request scope.** RFD 097 assigns `event_id` in the stream
  where existing entries are in scope; the active originating request is known
  in the same place.
- **Uniform serialization.** `ChatResponse` is an untagged serde enum, so adding
  a field to the payload would touch every variant.
  On the wrapper, `request_id` serializes beside `event_id` uniformly for every
  linked event kind.

### How the link is assigned

The stream stamps `request_id` when wrapping a payload into an `InternalEvent`,
extending RFD 097's insertion-time assignment:

- The active originating request is **turn-scoped**: a `TurnStart` clears it,
  and inserting a `ChatRequest` sets it.
- When a linkable payload (one of the five answering families) is inserted, the
  wrapper gets `request_id: Some(<active request id>)`.
- Non-linkable payloads get `request_id: None`.
- Linkable events inserted before any `ChatRequest` in the current turn (a
  malformed state whose stream-leading form `sanitize` already repairs) are
  inserted unlinked (`request_id: None`) rather than rejected or linked to a
  previous turn's request.

When a turn contains multiple `ChatRequest` events (interrupt replies), each
answering event links to the most recent `ChatRequest` at insertion time.
This records the same binding that positional readers assume.

Relying on position at insertion time is sound because events are appended in
causal order as they are produced; the explicit link frees later readers and
editors from relying on position.
Because an answer is produced in the same session as its request, both events
are persisted together, so a `request_id` never points at the transient
in-memory `event_id` of an unpersisted legacy entry (RFD 097).

### Storage

`request_id` serializes on the stream-entry object beside `event_id`:

```json
{
  "event_id": "k3m9x2a",
  "type": "chat_request",
  "timestamp": "2026-06-07 10:00:00.0",
  "content": "Review this PR"
}
```

```json
{
  "event_id": "p8q2r4b",
  "request_id": "k3m9x2a",
  "type": "chat_response",
  "timestamp": "2026-06-07 10:00:01.0",
  "message": "I'll review it."
}
```

```json
{
  "event_id": "w5t7y1c",
  "request_id": "k3m9x2a",
  "type": "tool_call_response",
  "timestamp": "2026-06-07 10:00:02.0",
  "id": "call_1",
  "content": "...",
  "is_error": false
}
```

The field name `request_id` does not collide with payload fields: tool and
inquiry payloads serialize a top-level `id` for immediate pairing, and RFD 097
uses `event_id` for wrapper identity to leave `id` untouched.
The field is omitted when `None`.
Older readers ignore the unknown key (forward-compatible), and entries without
`request_id` load as unlinked (backward-compatible).
Each linked event adds roughly 25 bytes on disk, which is negligible against
event content, and zero provider token cost because `request_id` is dropped
during provider conversion.

### Unlinked versus unresolved

A missing link and a broken link are distinct states with different handling:

- **Unlinked**: `request_id` is absent (`None`).
  This covers legacy events and event kinds that never carry a link.
  Readers handle unlinked events positionally.
- **Unresolved**: `request_id` is present (`Some`) but does not resolve to a
  valid target.
  A `request_id` is unresolved when:
  - no stream entry has that `event_id`;
  - the target entry is not a `ChatRequest`; or
  - RFD 097's load-time repair flagged the target ID as duplicated, making
    references to it ambiguous.

**JP never repairs an unresolved or missing `request_id` by position.** Falling
back to position would reintroduce the silent rebinding that `request_id`
prevents.

### Invariant enforcement

Each stream pass has one responsibility:

- **Load** preserves raw events verbatim, including unresolved links.
  Raw history is the user's file; loading does not rewrite it.
- **`sanitize`** does not infer, rewrite, or drop events based on `request_id`,
  matching RFD 097's rule keeping ID repair out of `sanitize`.
  Existing orphan repair for tool and inquiry pairs continues to operate on the
  payload `id` fields.
- **Provider projection** (`Thread::into_parts`) excludes events with an
  unresolved `request_id` from the provider request, closed over immediate tool
  pairs and reported as a diagnostic.
  Unlinked events are unaffected.
  See [Provider-projection exclusion](#provider-projection-exclusion) for
  ordering, pair closure, and diagnostics.
- **`jp conversation edit --events`** documents `request_id`: deleting a
  `ChatRequest` leaves its answer unresolved (and its raw events
  provider-invisible) until the user also deletes or relinks those events.
  An existing compaction summary covering the exchange still projects (see
  [Provider-projection exclusion](#provider-projection-exclusion)).

### Provider-projection exclusion

Resolution and exclusion run on the **raw stream, before compaction
projection**.
Compaction projection consumes `Compaction` entries, drops covered events, and
synthesizes summary entries that carry no `event_id`, so links must be resolved
while their targets still exist in the stream.
Running exclusion first is also safe for compaction: `TurnStart` and
`ChatRequest` events are never linked and never excluded, so exclusion cannot
shift turn numbering or suppress summary injection.

Compaction summary overlays are unaffected by exclusion: a summary covering an
excluded answer still projects.
Raw events with an unresolved link never reach the provider, while staleness of
the summary's derived text is handled separately (see Risks).

**Pair closure.** Exclusion is closed over immediate tool pairs: when either
half of a `ToolCallRequest`/`ToolCallResponse` pair is excluded, both halves are
excluded.
Matching uses the payload `id`, scoped to the turn and count-aware so it
tolerates providers that reuse a tool-call ID within a turn.
For streams written by JP, every event of an answer already shares one
`request_id` and is excluded as a group.
Manual edits, however, can split a pair across linked and unlinked halves;
without pair closure, the provider view would contain an orphaned tool call that
providers reject.
Inquiry pairs need no closure rule because the visibility allowlist excludes
inquiry events from provider input regardless of linking.
Closure applies only to the projected provider copy; raw history is never
rewritten.

**Diagnostics.** Projection returns a structured diagnostic alongside the
projected events rather than rendering output directly.
Each diagnostic records the excluded event IDs (including pair-closure
additions), the unresolved `request_id`, and the reason (missing target,
wrong-kind target, or duplicated target).
`jp_conversation` renders nothing; `jp_cli` surfaces the diagnostic on the
chrome channel ([RFD 048]) before the provider call and emits it to tracing.
Because JP writes tracing only to the log file by default, a log-only warning
would be invisible after a manual edit; the chrome channel is visible at default
verbosity.
Non-interactive query paths (title generation, compaction summaries) have no
chrome channel and emit the diagnostic to tracing only.

Every rendering conveys the excluded IDs, target ID, and reason, but the chrome
rendering (text or JSON) is human-facing prose rather than a stable machine
interface: under `--format json`, it uses the generic `{"message": …}` chrome
envelope.
Scripts must not parse it; a stable structured surface would require a separate
proposal built on the internal diagnostic type.

Surfacing the diagnostic before the provider call shifts the projection
boundary.
Instead of each provider invoking `Thread::into_parts` internally after `jp_cli`
hands off the query, the turn loop projects the thread via `jp_conversation`
before constructing the provider query, renders the diagnostic, and passes
already-projected parts to the provider.

### Backward compatibility

Legacy events without `request_id` remain unlinked on load.
JP does not infer links by position during normal reads or backfill them on
save.
New events appended to a legacy conversation are linked normally on insertion,
so features that consume request links must tolerate mixed streams by treating
unlinked events positionally.
Best-effort relinking of well-formed legacy turns belongs in an explicit repair
command rather than the read path (see Future Work).

## Drawbacks

- **The wrapper gains a kind-dependent optional field.** Unlike `event_id`,
  which every entry has, `request_id` applies only to the five linked event
  families.
  Tests, rather than the type system, enforce the field's presence rules.
- **Deleting a request shrinks provider context.** Deleting a request while
  keeping its answer excludes that answer's raw events from provider input,
  while an existing compaction summary covering the deleted exchange may still
  describe it until compaction is reset or regenerated.
  Exchanging silent rebinding for explicit exclusion is intentional, and the
  chrome-channel diagnostic (visible at default verbosity on the next query) and
  `edit --events` documentation surface the behavior to hand-editors.
- **Manual edits have one more field to maintain.** Moving an answer to a
  different request requires updating `request_id` by hand.
- **Additional `InternalEvent` serde surface.** Sequencing this change after RFD
  097's serde rewrite keeps it a single migration.
- **Providers stop owning projection.** Surfacing the diagnostic before the
  provider call moves the `Thread::into_parts` call site from the providers to
  the query caller, which is a wider mechanical change than adding the field
  itself.

## Alternatives

1. **Payload-level `request_id`** on `ChatResponse`, `ToolCallRequest`, and the
   other answering types.
   Rejected: requires every payload constructor and the `EventBuilder` and
   turn-coordinator pipeline to learn the request ID, modifies an untagged serde
   enum, and contradicts RFD 097's model of assigning identity fields where
   stream context exists.
2. **Keep positional binding.** Rejected: fails under structural edits to
   `events.json` and in turns with multiple `ChatRequest` events.
3. **Link only `ChatResponse`.** Rejected: leaves tool and inquiry events bound
   positionally, forcing any consumer that needs the whole answer to define the
   wider grouping itself.
4. **Positional fallback for unresolved links.** Rejected: reintroduces silent
   rebinding after manual edits.
5. **A payload-level pairing `id`, like tool calls use.** Rejected: adds a
   second ID namespace with per-kind matching rules instead of reusing the
   stream-wide `event_id` from RFD 097.

## Non-Goals

- **Participant attribution.** Which assistant produced an event belongs to a
  future multi-participant conversations RFD; this RFD provides the request
  grouping that design builds on.
- **Branching, undo, and compaction anchoring.** Each is a consumer of the link
  with its own RFD; nothing here implements them.
- **Immediate-request pairing.** `ToolCallResponse.id` and `InquiryResponse.id`
  semantics, along with the `sanitize` orphan repair built on them, are
  untouched.
- **Projection-created synthetic events.** Compaction projection synthesizes
  `ChatRequest`/`ChatResponse` summary pairs that exist only between projection
  and provider conversion; they are never persisted and carry no `request_id`.
  Because summary policies cover whole turns, a summarized `ChatRequest` takes
  its entire answer with it, so no surviving event outside the range can link
  into it.
  Summary overlays are likewise out of scope: exclusion never drops or rewrites
  a summary (see Risks).
- **Backfill and repair tooling.** See Future Work.

## Risks and Open Questions

- **Provider alternation after exclusion.** Excluding an unresolved answer can
  leave two `ChatRequest` events adjacent in provider input.
  Provider conversions already tolerate irregular shapes (and `sanitize` already
  deletes events), but implementation must validate each provider's conversion
  against a stream with an excluded group.
- **RFD 097 sequencing.** This design cannot land before RFD 097's wrapper and
  insertion-time assignment exist; the `Extends` relationship gates promotion
  accordingly.
- **Interrupt-reply binding.** Linking to the newest `ChatRequest` at insertion
  time matches positional assumptions, but interrupt-heavy turns should be
  exercised in tests to confirm the recorded binding matches user expectations.
- **Stale summaries can still describe deleted exchanges.** A summary covering a
  deleted request projects unchanged, so its LLM-generated text may continue to
  describe the deleted exchange even though the raw answer events are excluded.
  Detecting stale summaries belongs to compaction's planned migration to RFD 097
  ID anchors ([RFD 064]), and deciding whether to drop or regenerate a summary
  is an effectful operation outside this RFD's pure filtering.

## Future Work

- Add an explicit, user-invoked `jp conversation repair` (or `edit`-integrated)
  command for best-effort relinking: backfill `request_id` for legacy events in
  well-formed turns containing a single `ChatRequest`, and interactively relink
  unresolved events.
  Repair never runs on the read path.

## Implementation Plan

### Phase 1: Wrapper field and insertion-time stamping

- Add `request_id: Option<EventId>` to the `InternalEvent` wrapper and its serde
  (omit when `None`).
- Track the active originating request in `ConversationStream`; stamp the five
  linked families in every insertion path (`push`, `extend`, and the `TurnMut`
  flush).
- Regenerate affected snapshots and fixtures.
- Tests: linked kinds get stamped; non-linkable kinds stay unlinked; a linkable
  event inserted after a new `TurnStart` but before that turn's `ChatRequest`
  stays unlinked even when a previous turn contains a request; multi-request
  (interrupt) turns link to the newest request; round-trip preserves the field
  byte-for-byte.

Depends on RFD 097 (Phases 1-3).
Mergeable on its own: the field is written and preserved, not yet consumed.

### Phase 2: Resolution, exclusion, and diagnostics

- Implement unresolved-link detection on the raw stream (missing target,
  wrong-kind target, RFD 097-duplicated target), ordered before compaction
  projection in `Thread::into_parts`.
- Exclude unresolved-linked events, closed over immediate tool pairs (payload
  `id`, turn-scoped, count-aware).
- Move the `Thread::into_parts` call site from the providers to the query
  caller: the turn loop projects before building the provider query, and
  `ChatQuery` carries the projected parts plus the schema and config values
  providers extract from the raw thread before conversion.
- Return excluded-link diagnostics alongside `ThreadParts`; `jp_cli` renders
  them on the chrome channel and emits them to tracing.
  Non-interactive callers (title generation, compaction summaries) emit to
  tracing only.
- Tests: deleting a request excludes its whole answer; a hand-edited pair split
  across linked and unlinked halves (or across divergent `request_id`s) is
  excluded as a pair; unlinked legacy events are unaffected; a summarized range
  containing an excluded answer still projects its summary; provider conversions
  accept a stream with an excluded group; a default-verbosity run shows the
  diagnostic on the chrome channel.

Depends on Phase 1.

### Phase 3: Documentation

- Document `request_id` in `jp conversation edit --events` help, including the
  consequence of deleting a linked-to request: its raw answer events become
  provider-invisible, while an existing compaction summary covering the exchange
  may still describe it (pointing to `jp conversation compact --reset` for that
  case).
- Add glossary entries for "request link", "originating request", and "immediate
  request" in `docs/wiki/00dxybd-ubiquitous-language.md`.

Can land alongside Phase 2.

## References

- [RFD 097]: Stable Event Identifiers (defines `event_id` and load-time
  duplicate repair, and defers reference semantics to consumers).
- [RFD 047]: Editor and Path Access for Conversations (motivates the manual
  `events.json` edits the link must survive).
- [RFD 048]: Four-Channel Output Model (defines the chrome channel that carries
  the exclusion diagnostic).
- [RFD 064]: Non-Destructive Conversation Compaction (defines the compaction
  projection layer and owns summary-staleness handling).

[RFD 047]: 047-editor-and-path-access-for-conversations.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 097]: 097-stable-event-identifiers.md
