# RFD 097: Stable Event Identifiers

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-03
- **Extended by**: [RFD 098], [RFD 126]
- **Summary**: Assigns a stable, stream-unique identifier to every
  `ConversationStream` entry so references survive manual edits.

## Summary

This RFD assigns a stable, stream-unique identifier to every entry in a
conversation's event stream.
The identifier lives on the stream-entry wrapper so conversation events, config
deltas, and compaction overlays can be referenced by ID rather than by array
index.
Entries loaded from older files without an ID receive a random ID at load time,
which is persisted on the next save.

## Motivation

Stable event IDs do not add a user-facing feature on their own; they eliminate
positional-reference bugs when streams are edited and unblock features that
reference specific stream entries.

### Manual edits invalidate positional references

JP encourages users to edit `events.json` by hand via `jp conversation edit
--events`.
Common edits include:

- **Delete a digression.** Let a tangent play out for a few turns and remove
  those entries when it goes nowhere.
- **Rewind an in-flight bad query.** Cancel a request mid-stream, drop the
  partial response, edit the user message in the JSON, and restart with `jp q
  --no-edit`.
- **Drop noisy tool calls.** Remove tool requests and responses whose output
  pollutes the working context.

Each edit mutates the array structure and silently invalidates position-based
references between entries:

- Reordering entries changes what "turn 3" means without any signal.
- Copying an entry preserves its content but breaks identity-by-position.
- Deleting an entry shifts subsequent indices and silently invalidates every
  reference into that range.

Stable IDs make stream-entry identity intrinsic.
A reference either resolves to its target entry or, if the target was deleted,
fails with a detectable mismatch instead of silently aliasing another entry.

### Future features need stable references

Several planned features need to reference specific stream entries:
request/response event linking, branching, undo, and plugin event subscriptions.
Future sub-agent designs that track event-level provenance may also consume
event IDs ([RFD 051] operates at conversation granularity and does not require
them).
Without stable IDs, each feature either reinvents positional references and
inherits the same mutation hazards, or builds a parallel ID scheme of its own.
Assigning IDs once at the stream-entry level solves the problem for all of them.

Conversation compaction ([RFD 064]) anchors ranges by turn index and can migrate
to event-ID anchors in a follow-up change.
Until that migration, an event-removing transformation preserves compaction
overlays whose ranges lie entirely before the earliest removed turn and drops
overlays whose positional anchors may have shifted or lost covered content (see
`ConversationStream::retain`).

Migrating compaction to event-ID anchors refines that drop rule differently for
each policy kind:

- **Mechanical overlays** (reasoning and tool-call stripping) carry no derived
  content.
  Anchoring by ID lets projection apply the overlay to whichever covered events
  remain after a mutation, rebasing the overlay without dropping it.
- **Summary overlays** carry LLM-generated prose describing the events in their
  range.
  Removing any covered event makes that prose stale, and prose cannot be clipped
  to match the remaining events.
  Anchoring by ID provides precise staleness detection (the covered-ID set is no
  longer complete), after which the overlay must be either dropped or
  regenerated.
  Because regeneration is effectful (requiring a provider, model config, and
  async execution), it belongs in the imperative shell rather than the pure
  projection layer.

## Design

### User-facing behavior

Nothing changes for normal CLI usage.

For users who edit `events.json` by hand:

- Each stream entry has a short opaque `event_id` field.
- Editing an entry's content keeps its ID, so existing references stay valid.
- Duplicating an entry produces two entries with the same ID.
  On load, JP restores uniqueness in memory by assigning a new ID to the later
  occurrence and persists the repaired ID on the next save.
  Any reference to the duplicated ID is ambiguous, so reference resolution
  treats it as unresolved rather than binding to either copy.
- Deleting an entry invalidates references to it, and downstream features drop
  the dependent references.

`jp conversation edit` documents the `event_id` field and this copy and
reference behavior in its help text.

### Data model

The stream-entry identifier lives on the wrapper struct that holds every entry,
not on individual event types.
`InternalEvent` becomes a struct pairing `event_id` with a flattened payload
enum:

```rust
struct InternalEvent {
    event_id: EventId,
    payload: EventPayload,
}

enum EventPayload {
    Event(Box<ConversationEvent>),
    ConfigDelta(ConfigDelta),
    Compaction(Compaction),
    Unknown(Value),
}
```

Putting `event_id` on the wrapper guarantees at the type level that every stream
entry has an ID, including `Compaction` overlays and any future payload
variants.
Every item in the persisted event array is addressable by ID rather than by
position, whether or not a Rust consumer reads that ID.

Programmatic access is narrower than on-disk addressability: the
conversation-event iterators expose `event_id` only for the `ConversationEvent`s
they already yield.
Config deltas, compactions, and unknown entries carry IDs in `events.json` but
gain no new Rust accessors until a consumer RFD defines the raw-entry view it
needs.

`Unknown` keeps its forward-compatibility contract: entries with an unrecognized
`type` tag remain invisible to event iteration, config resolution, and
providers, while participating in uniqueness repair like any other entry.
Because `Unknown` retains the raw JSON object verbatim, deserialization extracts
`event_id` into the wrapper and stores the remaining object without it.
Serialization then writes `event_id` once so the entry round-trips losslessly.

`timestamp` stays on each payload variant because its provenance differs from
`event_id`.
The stream assigns `event_id` at insertion, whereas the event's producer reports
`timestamp`, and synthetic events deliberately preserve a source timestamp.

`event_id` and the flattened payload serialize into a single JSON object,
leaving the on-disk shape unchanged except for the added key:

```json
{ "event_id": "k3m9x2a", "type": "...", "timestamp": "...", ... }
```

The persisted JSON key is `event_id` rather than `id` because several event
kinds (`ToolCallRequest`, `ToolCallResponse`, `InquiryRequest`,
`InquiryResponse`) already serialize a top-level `id` when `ConversationEvent`
flattens its `kind` into the outer object.
Using `event_id` avoids key collisions and keeps older readers tolerant of the
unknown field.

`EventId` is an opaque newtype in `jp_conversation` that serializes
transparently as a string.
Generated IDs are 7 lowercase alphanumeric characters (base 36, ~78 billion
values), matching Git's short-ref length and providing ample headroom for
streams of ~10k entries with insertion-time uniqueness and load-time collision
repair (see [Storage-layer repair](#storage-layer-repair)).
The format is an internal generation convention, not a public contract or a
parsing constraint: nothing in the public surface relies on the character count,
alphabet, or generation method, and code in JP MUST NOT use `EventId` for
ordering, content addressing, or anything beyond stream-scoped identity.

`EventId` deserialization is strict: it accepts any non-empty string (preserving
hand-edited IDs that do not match the generated format) and rejects empty
strings.
Leniency for missing or empty IDs lives at the storage boundary: stream-entry
deserialization treats a missing or empty `event_id` as an instruction to assign
a fresh ID at load, matching the `InternalEvent` / `deserialize_config_delta`
compatibility path.

`EventId` is **not** a `jp_id::Id`.
Event IDs are not part of JP's CLI input or display contract: users do not pass
them as command arguments, JP does not print them outside raw-JSON contexts, and
they do not share the `jp_id` format contract of `ConversationId` and
`ProviderId`.
They do appear in `events.json` and, through [RFD 072], in the command-plugin
protocol, whose stability contract RFD 072 governs.
Because a legacy conversation that has never been re-saved receives fresh random
IDs on each load (see Drawbacks), a read-only plugin can observe different
`event_id` values for the same legacy entry across invocations until the stream
is persisted.
`event_id` is host-owned: a future write API (such as RFD 072's `push_events`)
appends payloads and lets the stream assign IDs at insertion, ignoring any
`event_id` supplied on a new event.

### ID assignment

`ConversationStream` owns the set of `EventId`s it has assigned and enforces
stream-scoped uniqueness whenever it wraps a payload into an `InternalEvent`.
Event and config-delta constructors (`ConversationEvent::new(kind, ts)`,
`ConversationEvent::now(kind)`, and `ConfigDelta` constructors) keep their
existing signatures and take no ID argument:

```rust
/// The entry IDs one conversation stream has handed out.
struct EventIds(HashSet<EventId>);

impl EventIds {
    /// Take `preferred`, or a generated ID when this set already holds it.
    fn claim(&mut self, preferred: EventId) -> EventId;

    /// A generated ID this set has not handed out.
    fn fresh(&mut self) -> EventId;
}

impl ConversationStream {
    /// Append a payload, returning the ID the stream assigned it.
    fn append(&mut self, payload: EventPayload) -> EventId;

    /// Append an entry from another stream, keeping its ID when free.
    fn adopt(&mut self, entry: InternalEvent) -> EventId;
}
```

Insertion returns the assigned `EventId` so callers can reference the new entry
without reading it back from the stream.
Every path that adds an `InternalEvent` routes through the stream's ID set:
`push`, `add_config_delta`, `add_compaction`, `extend`, `append_stream`,
`TurnMut::build`, `start_turn`, the synthetic insertions in
`normalize_turn_starts` and `sanitize_orphaned_tool_calls`, and projection's
injected entries (ephemeral; see [Projection views](#projection-views)).
Because the stream assigns IDs at insertion, uniqueness holds immediately rather
than waiting for a load-time pass.

Removing an entry from an in-memory stream does not free its `EventId` for reuse
during that stream's lifetime.
Reusing a deleted entry's ID in memory would allow a stale reference to rebind
silently to a new, unrelated entry.

Retirement is not persisted across loads.
Loading a stream rebuilds `EventIds` from the entries still present in
`events.json`, so a subsequent random draw could theoretically collide with an
ID deleted in an earlier session (1 in 36^7 per deleted ID).
Persisting tombstones would grow `events.json` without bound to prevent a
collision that no input can force, so consumers that resolve references must not
assume cross-load non-reuse.

**An entry copied from another stream keeps its ID.** Because uniqueness is
scoped to a single stream (see [Non-Goals](#non-goals)), an entry copied between
streams retains its `event_id` unless the destination stream has already
assigned that ID.
References that resolved against the source stream therefore continue to resolve
against a copy such as `jp conversation fork`.
By contrast, `Extend<ConversationEventWithConfig>` recomputes config deltas
against the destination stream's running config state rather than copying the
source entries, so those deltas are new entries and receive fresh IDs.

Tests and fixtures that need deterministic IDs construct `InternalEvent`
wrappers directly and pass them to the stream, which registers their IDs.
Production and synthetic insertion paths let the stream generate IDs; no
production signature takes a generator argument, and there is no thread-local
RNG.
Synthetic entries that preserve a source timestamp (such as a `TurnStart`
adopting the first chat request's timestamp, or a synthetic `ToolCallResponse`
adopting its request's timestamp) continue to set that timestamp on the payload
while the stream assigns a fresh `event_id`.

When loading from storage, entries with a unique `event_id` keep it, and entries
without an `event_id` receive a random `EventId` in memory that is persisted on
the next save (see [Storage-layer repair](#storage-layer-repair)).

### Projection views

`event_id` stability is a property of the persisted raw stream in `events.json`.
Compaction projection builds an ephemeral provider view by transforming a copy
of the stream without removing original entries from `events.json`.

An entry retained in the projected view keeps its original `event_id`, even when
a mechanical policy modifies its projected content.
That ID still identifies the original entry in `events.json`, not a separate
stored version of the projected content.

Entries synthesized for the projection, such as the summary `TurnStart` and
`ChatRequest` / `ChatResponse` pair, exist only in the projected view and have
no counterpart in `events.json`.
The projected stream wraps them and assigns fresh `EventId`s from its own
`EventIds` set, ensuring synthetic IDs neither collide with retained entries nor
get reassigned if further entries are appended to the projected stream.
These synthetic IDs are ephemeral: they carry no stability contract across
projections, are never persisted or exposed through storage or plugin APIs, and
MUST NOT be used as references into `events.json`.

Because both stored and projection-only entries use `EventId`, the ID value
alone does not indicate whether a corresponding stored entry exists.

### Dependency

`EventId::random()` uses `getrandom`, promoted from a transitive dependency to a
direct workspace dependency and used by `jp_conversation`.
`getrandom` is a thin wrapper around the OS RNG with no distribution machinery
or allocator dependencies.
Heavier crates (`rand`, `uuid`, `ulid`, `nanoid`) add surface area for
properties (distributions, time-sortability, universal uniqueness) that
`EventId` does not need.

OS RNG failure is fatal: `EventId::random()` panics with a clear message rather
than returning a `Result`.
`getrandom` fails only when the OS entropy source is broken, and threading a
`Result` through every append API (including `Extend`, which cannot return one)
would complicate the call graph for an unrecoverable condition.
The append APIs remain infallible.

### Storage-layer repair

A stored entry's `event_id` is optional on the wire (`StoredEvent` carries
`Option<EventId>`) because legacy entries have no ID and hand-edited files can
contain duplicate IDs.
`ConversationStream::from_parts` (and `from_legacy_events`, which delegates to
it) assigns and deduplicates every entry's ID when building the stream:

```txt
reserve every id the file carries;
for each entry, in order:
  no id            -> assign a generated one
  id not seen yet  -> keep it
  id already seen  -> assign a generated one, and record the id as duplicated
```

Reserving all existing IDs from the file before generating any new ones prevents
a generated ID from colliding with an ID that appears later in the file.
Because `StoredEvent` is only converted into `InternalEvent` inside
`from_parts`, stream-scoped uniqueness is enforced by construction rather than
by a separate post-load pass that a future load path could skip.

ID repair is separate from `ConversationStream::sanitize()`, which enforces
higher-level stream invariants (orphaned tool and inquiry responses, leading
non-user events, and turn-start normalization) on a stream whose `EventId`s are
already assigned.

Repair restores ID uniqueness in memory (and on the next save), but it cannot
recover the intended target of a reference.
When a manual edit duplicates an ID, whether by copy-pasting an entry or moving
a copy ahead of the original, there is no way to know which copy a pre-existing
reference meant, and keeping the first occurrence's ID does not make the
reference valid.
Reference resolution in future overlay features therefore treats any ID that was
duplicated at load as **unresolved**, just like a deleted ID.
Copying, reordering, or deleting an entry always produces a detectable mismatch
rather than silently binding to the wrong entry.

`ConversationStream` records IDs that were duplicated during loading in
load-scoped state, exposed via `duplicated_event_ids()`.
Once the repaired stream is saved and reloaded, the file contains unique IDs and
this set is empty.

This duplicated-ID set applies only to the load that produced it and is not
copied when entries move to another stream via `append_stream` or `Extend` (for
example, during `jp conversation fork`).
Ambiguity is a property of the loaded file, so a consumer resolving a reference
queries the stream that loaded that file rather than a stream copied from it.
Propagating the set across streams is also ill-defined because no consumer
specifies how inherited ambiguity should behave, and `Extend` recomputes config
deltas with new IDs rather than copying them.

Features that introduce reference-bearing entries must consume
`duplicated_event_ids()` during the same load cycle and resolve or drop
ambiguous references before saving (or refuse the write).
Saving a repaired stream without cleaning up references would discard the
ambiguity record and let a stale reference bind silently to the surviving copy
on the next load.
Repair itself logs a warning for each regenerated ID and returns no
caller-facing report; overlay RFDs define their own user-facing behavior for
ambiguous and orphaned references.

Existing `sanitize` steps continue to work unchanged.
Future overlay types that anchor to event IDs add a cleanup step over the IDs of
all stream entries (every `InternalEvent`, regardless of payload kind):

```txt
if overlay.anchor_id was duplicated at load, or
   overlay.anchor_id ∉ { id of every stream entry } { drop overlay }
```

`ConversationStream` does not expose a public accessor for the set of live entry
IDs until a consumer (compaction, sub-agent provenance, plugin subscriptions, or
interactive editing) defines the API it needs.
`duplicated_event_ids()` is exposed immediately because its ambiguity record
only exists until the stream is saved.

### Storage

Each entry in `events.json` gains a short `event_id` key, adding negligible
on-disk overhead even for large conversations.
The format is both forward- and backward-compatible: older readers ignore the
unknown `event_id` field, and legacy files without `event_id` receive random IDs
at load time that are persisted on the next save.

## Drawbacks

- **One-time structural change to `InternalEvent`.** `InternalEvent` becomes a
  struct with a flattened payload enum, requiring a rewrite of its custom
  `Serialize` and `Deserialize` implementations to preserve the `type` tag and
  base64 encode/decode hooks, plus updates to the iteration views to expose
  `event_id`.
  Because the stream assigns `event_id` on the wrapper at insertion,
  `ConversationEvent::new` and `ConfigDelta` call sites in tests stay unchanged;
  only fixtures that assert on a specific ID construct the wrapper directly.
  The custom serde implementation is the main risk and should be spiked against
  existing fixtures to verify a byte-for-byte round-trip (modulo `event_id`).
- **Extra load-time pass.** `from_parts` and `from_legacy_events` run uniqueness
  enforcement on every load.
  The pass is cheap (`HashSet` lookups over the stream's entries), but it adds
  work to every conversation load.
- **Delayed repair when copy-pasting entries by hand.** A user who duplicates an
  entry in `events.json` sees both copies with the same ID until the next load,
  when JP assigns a new ID to the second copy and marks existing references to
  that ID as unresolved.
- **Unstable in-memory IDs for unsaved legacy files.** Loading a legacy file
  twice without saving produces two different sets of random IDs.
  Nothing consumes those IDs without persisting first, though a future read-only
  consumer would need the stream saved once to stabilize them.
- **`timestamp` remains duplicated across payload variants.** Unlike `event_id`,
  `timestamp` stays on each payload variant rather than moving to
  `InternalEvent`.
  Every variant already carries a timestamp, its provenance is the event
  producer rather than the stream, and lifting it to the wrapper would cause
  churn across every event constructor and timestamp read site.
  Revisit only if a third shared field forces the wrapper to grow.

## Alternatives

**1. Keep position-based references and drop dependent overlays on mutation.**
This localizes the cost to each feature that anchors to stream entries, but it
does not generalize: every future feature has to reinvent mutation handling, and
each implementation is a new opportunity for positional aliasing bugs.

**2. Assign IDs only to `TurnStart` events.** This is a smaller schema change,
but any feature that needs to reference a non-turn entry would later have to add
IDs to additional event types.

**3. Content-addressed IDs (hash of the entry's serialized content).** Hashing
requires no random number generator, but editing an entry's content changes its
ID, breaking the manual-edit workflow that motivates this RFD.

**4. Longer or structured ID formats (UUIDv4, UUIDv7, ULID, `jp_id`
membership).** These formats are over-specified for a per-conversation,
internal, opaque identifier.
UUIDs and ULIDs add bytes for universal uniqueness and time-sortability, neither
of which has a consumer here because array order defines stream order.
Using `jp_id` would leak an internal storage detail into the public ID format
contract.

**5. Generate the ID inside the event constructor.** Putting `event_id` on
`ConversationEvent` and `ConfigDelta` requires constructors to supply it, either
through an explicit parameter (causing churn at every call site) or through a
thread-local RNG (introducing hidden non-determinism in constructors and extra
test plumbing).
Assigning the ID on the `InternalEvent` wrapper at insertion avoids both
problems: constructors stay unchanged, there is no hidden RNG state, and the
stream can enforce uniqueness immediately against its existing IDs.

**6. Lift `timestamp` onto the `InternalEvent` wrapper alongside `event_id`.**
Every payload variant already carries a timestamp, so moving `timestamp` to the
wrapper adds no new type-level guarantee while reintroducing constructor and
read-site churn across the codebase.
Revisit only if a third shared field is added to all stream entries.

## Non-Goals

- **Cryptographic identity.** IDs are not tamper-evident or unforgeable.
- **Universal uniqueness.** Uniqueness is required only within a single
  conversation's stream.
  Cross-conversation, cross-workspace, and cross-machine collisions are out of
  scope.
- **Time-sortability.** Array order defines stream order.
  IDs do not encode time, and code MUST NOT use them for ordering.
- **Stable identity for projected views.** Synthetic entries created during
  projection receive ephemeral IDs with no stability contract and MUST NOT be
  referenced (see [Projection views](#projection-views)).
- **Content-addressing or deduplication.** Two entries with identical content
  receive distinct IDs.
  [RFD 067] covers deduplication separately.
- **Streaming or replication semantics.** ID assignment and repair are local to
  a single conversation file.
- **Surfacing repair to users or callers.** Load-time ID repair logs a warning
  and returns no structured report.
  Overlay RFDs define their own surfacing for orphaned and ambiguous references.
- **Migrating existing reference sites.** Features that consume event IDs
  (compaction, sub-agent provenance, plugin event subscriptions, interactive
  stream editing) migrate their own reference layouts in follow-up RFDs.
- **Plugin-visible event-identity semantics beyond JSON exposure.** [RFD 072]
  governs what plugins observe in `read_events`; this RFD only adds `event_id`
  to the serialized JSON format.

## Risks and Open Questions

- **Repair cost on load.** Uniqueness repair adds a pass over stream entries on
  every load.
  The pass is linear in the number of entries, and can be revisited if profiling
  shows measurable overhead.
- **Fixture migration scope.** Keeping the event constructors unchanged leaves
  most test call sites untouched, concentrating churn in the `InternalEvent`
  serde rewrite, the iteration-view updates, and snapshots that gain an
  `event_id` key.
  A small test helper for constructing `InternalEvent` wrappers with readable
  fixed IDs keeps ID-specific tests concise.

## Implementation Plan

### Phase 1: `EventId` type and dependency

- Promote `getrandom` to a direct workspace dependency.
- Add `EventId` newtype in `jp_conversation` as an internal opaque ID with no
  `jp_id` membership.
- Implement `EventId::random()` on top of `getrandom`, transparent string serde,
  `Display`/`Debug`, and a test-fixture helper for readable fixed IDs.
- Tests: serde round-trip; `EventId` deserialization rejects an empty string
  (strict); non-format but non-empty strings are preserved as-is; two `random()`
  calls produce distinct IDs.

This phase is independent and can merge on its own.

### Phase 2: `InternalEvent` wrapper and stream-assigned IDs

- Make `InternalEvent` a struct with `event_id: EventId` and a flattened
  `EventPayload` enum (`Event` / `ConfigDelta` / `Compaction` / `Unknown`).
- Rewrite `InternalEvent`'s `Serialize` and `Deserialize` implementations for
  the struct-plus-flatten shape, preserving the `type` tag, the base64
  encode/decode hooks, and lossless round-tripping of `Unknown` entries (extract
  `event_id` from the raw object on load; write a single `event_id` on save).
  Confirm a byte-for-byte round-trip against existing fixtures, modulo the new
  key.
- Assign `event_id` on insertion by routing every `InternalEvent` creation
  through the stream (`push`, `add_config_delta`, `add_compaction`, `extend`,
  `append_stream`, `TurnMut::build`, `start_turn`, and the synthetic insertions
  in `normalize_turn_starts` and `sanitize_orphaned_tool_calls`), backed by the
  stream's `EventIds` set.
  An entry copied from another stream keeps its ID unless the destination stream
  has already assigned that ID.
- Expose `event_id` on the iteration views for the `ConversationEvent`s they
  already yield; config deltas, compactions, and unknown entries gain no new
  programmatic surface and remain addressable by ID in the persisted JSON.
- On deserialization, treat a missing or empty `event_id` as an instruction to
  assign a fresh ID at load, while keeping `EventId` itself strict.
- Regenerate the snapshots and fixtures that gain an `event_id` key in the same
  change so the phase lands green.
- Verify no test relies on positional references that stable IDs would break.

Depends on Phase 1.

### Phase 3: Storage-layer uniqueness repair

- Give the wire form (`StoredEvent`) an `Option<EventId>` so an entry read from
  storage has no stream identity until `from_parts` assigns it.
- Assign and deduplicate every entry's ID in `from_parts` as the stream is
  built, recording duplicated values in load-scoped state exposed via
  `duplicated_event_ids()`.
  Delegate `from_legacy_events` to `from_parts`, and keep this logic out of
  `sanitize()`.
- Test: loading a stream with two entries sharing an explicit fixed ID keeps the
  first, reassigns the second, and reports the ID in `duplicated_event_ids()`.
- Test: loading a legacy file with no `event_id` fields assigns fresh IDs to all
  entries.
- Test: a scripted generator forced to repeat an existing ID retries until it
  draws an unused ID.

Depends on Phase 2.

### Phase 4: Documentation

- Update `jp conversation edit --events` help text with the duplicate-ID note
  (using `event_id`): a copy receives a new ID on the next load, and any
  reference to a duplicated ID becomes unresolved.
- Update the data model section of `docs/architecture/` if applicable.
- Add a glossary entry for "Event ID" in
  `docs/architecture/ubiquitous-language.md`.

Can land alongside Phase 3.

## References

- [RFD 047]: Editor and Path Access for Conversations (motivates manual editing
  of `events.json`)
- [RFD 051]: Sub-Agent Workflows (operates at conversation granularity; not an
  event-ID consumer)
- [RFD 054]: Split Conversation Config and Events (defines the storage shape
  this RFD modifies)
- [RFD 064]: Non-Destructive Conversation Compaction (candidate consumer;
  anchors ranges by turn index)
- [RFD 067]: Resource Deduplication (covers content deduplication separately)
- [RFD 072]: Command Plugin System (governs plugin-protocol exposure of
  `ConversationEvent` JSON)

[RFD 047]: 047-editor-and-path-access-for-conversations.md
[RFD 051]: 051-sub-agent-workflows-with-local-tools.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[RFD 072]: 072-command-plugin-system.md
[RFD 098]: 098-request-response-event-linking.md
[RFD 126]: 126-turn-summaries.md
