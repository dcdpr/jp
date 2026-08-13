# RFD 098: Request-Response Event Linking

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-07
- **Extends**: [RFD 097]

## Summary

The events that answer a chat request carry no explicit reference to the request
they answer; the relationship is inferred from position within a turn.
This RFD adds a **request link**: an optional `request_id` field on the
stream-entry wrapper, holding the `event_id` (introduced in [RFD 097]) of the
originating `ChatRequest`.
The stream stamps the link at insertion, so it is persisted and survives
reordering and deletion as a *detectable* reference rather than a positional
guess.
This RFD defines which event kinds are linked, where the link lives on disk, and
what every reader does with a missing or dangling link.

## Terminology

- **Originating request**: the `ChatRequest` whose answer is being produced.
- **Immediate request**: the paired `ToolCallRequest` or `InquiryRequest` that a
  `ToolCallResponse` or `InquiryResponse` matches via its existing payload `id`
  field.
- **Request link**: the `request_id` reference from an answering event to its
  originating request.

The request link is a new, separate concept.
`ToolCallResponse.id` and `InquiryResponse.id` keep their existing
immediate-request pairing semantics unchanged; this RFD does not touch them.

## Motivation

A conversation stream interleaves requests — chat requests from the human —
with the events that answer them: chat responses, plus the tool and inquiry
events produced while answering.
The binding between an answering event and its originating request is currently
positional: a reader assumes the event belongs to the most recent request in the
same turn.

Positional binding is fragile under exactly the hand edits JP encourages on
`events.json`.
Deleting a digression, rewinding an in-flight query, or dropping a noisy tool
call can silently change which request a response appears to answer.
It is also ambiguous even without edits: a turn can contain more than one
`ChatRequest` (interrupt replies inject additional requests mid-turn), so "the
request of this turn" is not always a single event.
Several proposed capabilities — branching, undo, compaction anchoring, faithful
turn reconstruction, and multi-participant conversations — need to know
unambiguously which request a given event answers, and cannot rely on position
surviving a structural edit.

RFD 097 gives every stream entry a stable `event_id` but deliberately leaves
reference semantics to its consumers, requiring each reference-bearing feature
to define its own orphan and ambiguity handling.
This RFD is one such consumer: it records the answer-to-request relationship as
an explicit `event_id` reference and defines that handling.

## Design

### Linked event families

Each event produced while answering a `ChatRequest` carries a request link to
that `ChatRequest`:

- `ChatResponse` (message, reasoning, and structured variants)
- `ToolCallRequest`
- `ToolCallResponse`
- `InquiryRequest`
- `InquiryResponse`

Not linked:

- `TurnStart`, `ChatRequest` — they open scopes, they don't answer one
- `ConfigDelta`, `Compaction` — global entries, not part of any answer
- unknown (forward-compatibility) events — opaque, passed through verbatim

All five answering families link to the *originating* `ChatRequest`, not to
their immediate request.
Tool and inquiry events keep their immediate pairing through the existing
payload `id`; the request link groups the whole answer — response, tool
round-trips, inquiries — under the request that caused it.
Linking all five here, rather than only `ChatResponse`, keeps the invariant in
one place, so a consumer that needs the whole answer — participant attribution,
branching, faithful turn reconstruction — takes the grouping as-is instead of
redefining it.

### Where the link lives

The link is a wrapper-level field, next to `event_id` on the stream-entry
wrapper defined by RFD 097:

```rust
struct InternalEvent {
    event_id: EventId,
    request_id: Option<EventId>,
    payload: EventPayload,
}
```

Wrapper-level, not payload-level, for the same reasons RFD 097 put `event_id` on
the wrapper:

- **Constructors stay unchanged.** `ChatResponse`, `ToolCallRequest`, and the
  other payload types gain no field and no constructor argument.
  The `EventBuilder` in `jp_llm` and the turn coordinator in `jp_cli`, which
  construct payloads with no stream context, need no plumbing to learn the
  request's ID.
- **The stream is the one place that knows the scope.** IDs are assigned where
  the stream's existing entries are in scope (RFD 097); the active originating
  request is known in exactly the same place.
- **No payload serde churn.** `ChatResponse` is an untagged serde enum;
  injecting a field into every variant is invasive.
  On the wrapper, `request_id` serializes beside `event_id` uniformly for every
  linked kind.

### How the link is assigned

The stream stamps `request_id` when it wraps a payload into an `InternalEvent`,
extending RFD 097's insertion-time assignment:

- The active originating request is **turn-scoped**: a `TurnStart` clears it,
  and inserting a `ChatRequest` sets it.
- When a linkable payload (the five families above) is inserted, the wrapper
  gets `request_id: Some(<the active request's id>)`.
- Non-linkable payloads get `request_id: None`.
- Linkable events inserted before any `ChatRequest` in the current turn (a
  malformed state; `sanitize` already repairs its stream-leading form) are
  inserted unlinked rather than rejected — never linked to a previous turn's
  request.

When a turn contains multiple `ChatRequest`s (interrupt replies), answering
events link to the newest request at their insertion time.
This is the binding today's positional readers *assume*; the link makes it
persistent and explicit.

Stamping at insertion time relies on position — and that is sound: position is
trustworthy at creation, when events are appended in causal order.
The point of the link is that *later* readers and editors no longer have to
trust position.
Note that a link is always created in the same session as its target (an answer
is produced after its request, in the same run), so links never point at RFD
097's transient in-memory IDs of never-persisted legacy entries: link and target
are persisted together.

### Storage

`request_id` serializes on the stream-entry object, beside `event_id`:

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

The name `request_id` collides with nothing: the tool and inquiry families
serialize a top-level `id` (their immediate pairing), and RFD 097 chose
`event_id` for the wrapper identity precisely to leave `id` alone.
The field is omitted when `None`.
The format is forward-compatible (older readers ignore the unknown key) and
backward-compatible (files without it load as unlinked, see below).

### Unlinked versus unresolved

Two distinct states, with distinct handling:

- **Unlinked**: the field is absent.
  Legacy events, and event kinds that never link.
  Unlinked events keep today's positional reader behavior unchanged.
- **Unresolved**: the field is present but does not bind.
  A `request_id` is unresolved when:
  - no stream entry has that `event_id`;
  - the target exists but is not a `ChatRequest`;
  - the target ID was flagged as duplicated by RFD 097's load-time repair
    (references to a duplicated ID are ambiguous and treated as unresolved, per
    RFD 097).

**JP never repairs an unresolved or missing `request_id` by position.** Falling
back to position would reintroduce exactly the silent rebinding this RFD exists
to eliminate.

### Invariant enforcement

Each stream pass has one job:

- **Load** preserves raw events verbatim, including unresolved links.
  Raw history is the user's file; loading does not rewrite it.
- **`sanitize`** neither infers, rewrites, nor drops based on `request_id`.
  This mirrors RFD 097 keeping ID repair out of `sanitize`.
  The existing orphan repair for tool and inquiry pairs continues to operate on
  the payload `id` fields, unchanged.
- **Provider projection** (`Thread::into_parts`) excludes events with an
  *unresolved* request link from the provider request, closed over immediate
  tool pairs and reported as a diagnostic.
  Unlinked events are unaffected.
  The ordering, closure rule, and diagnostic surface are defined in
  [Provider-projection exclusion](#provider-projection-exclusion) below.
- **`jp conversation edit --events`** documents the field: deleting a
  `ChatRequest` leaves its answer unresolved (and therefore its raw events
  provider-invisible) until the user also deletes or relinks those events.
  An existing compaction summary covering the exchange still projects (see
  [Provider-projection exclusion](#provider-projection-exclusion)).

### Provider-projection exclusion

Resolution and exclusion run on the **raw stream, before compaction
projection**.
This order is forced, not chosen: projection consumes `Compaction` entries,
drops covered events, and synthesizes summary entries that carry no `event_id`,
so links can only be resolved while their targets still exist.
It is also safe: `TurnStart` and `ChatRequest` events are never linked and never
excluded, so exclusion cannot shift turn numbering or suppress summary
injection.

Compaction summary overlays are unaffected by exclusion: a summary covering an
excluded answer still projects.
The raw invariant holds either way — unresolved-linked events never reach the
provider — but the summary's derived text is a separate concern (see Risks).

**Pair closure.** Exclusion is closed over immediate tool pairs: when either
half of a `ToolCallRequest`/`ToolCallResponse` pair is excluded, both halves
are.
Matching uses the payload `id`, scoped to the turn and count-aware, tolerating
providers that reuse one tool-call ID within a turn.
For streams JP wrote itself this changes nothing — every event of an answer
shares one `request_id`, so the group is already excluded whole.
Hand edits, however, can split a pair across linked and unlinked halves; without
closure the provider view would contain an orphaned tool call, which providers
reject.
Inquiry pairs need no closure rule: inquiry events are excluded from provider
input by the visibility allowlist regardless of linking.
Closure applies to the projected provider copy only; raw history is never
rewritten.

**Diagnostics.** Exclusion is reported as data, not output.
Projection returns a diagnostic alongside the projected events, naming the
excluded events and the missing or ambiguous target.
`jp_conversation` renders nothing; `jp_cli` surfaces the diagnostic on the
chrome channel ([RFD 048]) before the provider call, and also emits it to
tracing.
By default JP writes tracing only to the log file, so a log-only warning would
be invisible in exactly the hand-edit workflow this RFD is written for; the
chrome channel is visible at default verbosity.

The diagnostic is data internally and prose externally.
The projection-level diagnostic names, at minimum: the excluded event IDs
(including pair-closure additions), the unresolved `request_id`, and the reason
(missing target, wrong-kind target, or duplicated target).
Every rendering must convey this information, but the chrome rendering — text
or JSON — is not a stable machine interface: under `--format json` it appears
as the generic `{"message": …}` chrome envelope, like all chrome.
Scripts must not parse it; a stable structured surface, if ever needed, would be
a separate proposal built on the internal diagnostic type.

Making the diagnostic precede the provider call changes a boundary: today each
provider invokes `Thread::into_parts` internally, after `jp_cli` has handed off
the query.
This design moves projection to the query caller: the turn loop projects the
thread via `jp_conversation` before constructing the provider query, renders the
diagnostic, and providers receive already-projected parts — they no longer
invoke projection themselves.
Non-interactive query paths (title generation, compaction summaries) have no
chrome channel and emit the diagnostic to tracing only.

### Backward compatibility

Legacy events without `request_id` remain unlinked on load.
JP does not infer links by position during normal reads, and does not backfill
on save: a value JP never knew is not invented.
New events appended to a legacy conversation are linked normally from their
first insertion.
Features that consume request links must tolerate mixed streams by treating
unlinked events as they are treated today — positionally.
A future explicit repair command may offer best-effort relinking of well-formed
legacy turns; that is tooling, not read-path behavior (see Future Work).

## Drawbacks

- **The wrapper gains an optional, kind-dependent field.** Unlike `event_id`,
  which every entry has, `request_id` is meaningful only for the five linked
  families.
  The type system no longer guarantees the field's presence rules; tests do.
- **A bad edit now shrinks provider context.** Deleting a request while keeping
  its answer excludes that answer's raw events from provider input; an existing
  compaction summary covering the deleted exchange may still describe it until
  compaction is reset or regenerated.
  That is the intended, detectable alternative to silently rebinding the answer
  to the wrong request — but it is a behavior change a hand-editor must learn,
  mitigated by the chrome-channel diagnostic (visible at default verbosity on
  the next query) and the `edit --events` documentation.
- **Hand edits have one more field to maintain.** Moving an answer to a
  different request means updating `request_id` by hand.
- **Another `InternalEvent` serde change.** Small, and it rides on the serde
  rewrite RFD 097 already performs; sequencing after RFD 097 keeps it one
  migration.
- **Providers stop owning projection.** Surfacing the diagnostic before the
  provider call moves the `Thread::into_parts` call site from the providers to
  the query caller — a wider mechanical change than the field itself, and the
  real cost of the visibility promise.

## Alternatives

1. **Payload-level `request_id`** on `ChatResponse`, `ToolCallRequest`, etc.
   Rejected: it requires every payload constructor and the `EventBuilder` /
   turn-coordinator pipeline to learn the request's ID, touches an untagged
   serde enum, and contradicts RFD 097's model of assigning identity fields
   where stream context exists.
2. **Keep positional binding.** The status quo; its failure under structural
   edits is the motivation.
3. **Link only `ChatResponse`.** Smaller, but tool and inquiry events would
   still bind to their request positionally, and any consumer needing the whole
   answer would have to define the wider grouping itself — same invariant, two
   homes.
4. **Positional fallback for unresolved links.** Undercuts the RFD: an edit
   would again silently rebind answers.
5. **A payload-level pairing `id`, like tool calls use.** Adds a second ID
   namespace with per-kind matching rules instead of reusing the stream-wide
   `event_id` addressing RFD 097 was built to provide.

## Non-Goals

- **Participant attribution.** Which assistant produced an event is a concern
  for a future multi-participant conversations RFD; this RFD provides the
  request grouping such a design builds on.
- **Branching, undo, and compaction anchoring.** They are consumers of the link,
  each with its own RFD; nothing here implements them.
- **Immediate-request pairing.** `ToolCallResponse.id` / `InquiryResponse.id`
  semantics, and the `sanitize` orphan repair built on them, are untouched.
- **Projection-created synthetic events.** Compaction projection synthesizes
  `ChatRequest`/`ChatResponse` summary pairs that exist only between projection
  and provider conversion; they are never persisted and carry no `request_id`.
  This scoping is safe, not merely convenient: summary policies cover whole
  turns, so a summarized `ChatRequest` takes its entire answer with it — a
  surviving event outside the range cannot dangle into it.
  Summary *overlays* are likewise out of scope: exclusion never drops or
  rewrites a summary (see Risks).
- **Backfill and repair tooling.** See Future Work.

## Risks and Open Questions

- **Provider alternation after exclusion.** Excluding an unresolved answer can
  leave two `ChatRequest`s adjacent in provider input.
  Provider conversions already tolerate irregular shapes (and `sanitize` already
  deletes events), but implementation must validate each provider's conversion
  against a stream with an excluded group.
- **RFD 097 sequencing.** This design cannot land before RFD 097's wrapper and
  insertion-time assignment exist; the `Extends` relationship gates promotion
  accordingly.
- **Interrupt-reply binding.** Linking to the newest `ChatRequest` at insertion
  time matches current positional assumptions, but interrupt-heavy turns should
  be exercised in tests to confirm the recorded binding matches user intuition.
- **Stale summaries can still describe deleted exchanges.** A summary covering a
  deleted request projects unchanged; its LLM-generated text may keep describing
  the deleted exchange even though the raw answer events are excluded.
  Precise staleness detection is compaction's planned migration to RFD 097 ID
  anchors — [RFD 064]'s call, per RFD 097 — and the drop-or-regenerate
  decision is effectful, belonging to the imperative shell rather than this
  RFD's pure filtering.

## Future Work

- An explicit `jp conversation repair` (or `edit`-integrated) command that
  offers best-effort relinking: backfill `request_id` for legacy events in
  well-formed turns containing exactly one `ChatRequest`, and interactive
  relinking of unresolved events.
  Explicit, user-invoked, and never part of the read path.

## Implementation Plan

### Phase 1 — Wrapper field and insertion-time stamping

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

Depends on RFD 097 (Phases 1–3).
Mergeable on its own: the field is written and preserved, not yet consumed.

### Phase 2 — Resolution, exclusion, and diagnostics

- Implement unresolved-link detection on the raw stream (missing target,
  wrong-kind target, RFD 097-duplicated target), ordered before compaction
  projection in `Thread::into_parts`.
- Exclude unresolved-linked events, closed over immediate tool pairs (payload
  `id`, turn-scoped, count-aware).
- Move the `Thread::into_parts` call site from the providers to the query
  caller: the turn loop projects before building the provider query, and
  `ChatQuery` carries the projected parts plus the schema and config values
  providers currently extract from the raw thread before conversion.
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

### Phase 3 — Documentation

- `jp conversation edit --events` help: the `request_id` field, and the
  consequence of deleting a linked-to request — its raw answer events become
  provider-invisible, while an existing compaction summary covering the exchange
  may still describe it; point at `jp conversation compact --reset` for that
  case.
- Glossary entries for "request link", "originating request", and "immediate
  request" in `docs/architecture/ubiquitous-language.md`.

Can land alongside Phase 2.

Cost: roughly 25 bytes per linked event on disk — negligible against event
content.
Zero provider token cost: the field is dropped during provider conversion.

## References

- [RFD 097] — Stable Event Identifiers (the `event_id` this RFD references;
  defines load-time duplicate repair and defers reference semantics to consumers
  like this RFD)
- [RFD 047] — Editor and Path Access for Conversations (motivates the manual
  `events.json` edits the link must survive)
- [RFD 048] — Four-Channel Output Model (the chrome channel that carries the
  exclusion diagnostic)
- [RFD 064] — Non-Destructive Conversation Compaction (the projection layer
  scoped out in Non-Goals; owns summary-staleness handling)

[RFD 047]: 047-editor-and-path-access-for-conversations.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 097]: 097-stable-event-identifiers.md
