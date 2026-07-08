# RFD 097: Stable Event Identifiers

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-03

## Summary

Every entry in a conversation's event stream carries a stable identifier, unique
within its stream.
The identifier lives on the stream-entry wrapper, so every kind of entry
(conversation events, config deltas, compaction overlays, and any future kind)
is addressable by a stable ID instead of by position.
Future reference-bearing entries and overlays reference entries by ID rather
than by position.
Loaded entries without an ID get a fresh random ID assigned at load time and
persisted on the next save.

## Motivation

This is a technical change.
It does not add a user-facing feature today; it removes a class of correctness
problem and unblocks several future features.

### Manual edits are unsafe today

JP encourages users to edit `events.json` by hand via `jp conversation edit
--events`.
In practice, that workflow has three common shapes:

- **Delete a digression.** Let a tangent play out for a few turns, decide it
  didn't go anywhere useful, and remove those entries.
- **Rewind an in-flight bad query.** Cancel a request mid-stream, drop the
  partial response, edit the user message in the JSON, and restart with `jp q
  --no-edit`.
- **Drop noisy tool calls.** Remove tool requests/responses whose output is
  polluting the working context going forward.

Each of these is a structural mutation.
Position-based references between entries are silently invalidated by exactly
this kind of edit:

- Reordering entries changes what "turn 3" means without any signal.
- Copying an entry preserves its content but breaks identity-by-position.
- Deleting an entry silently invalidates every reference into that range.

Stable IDs make stream-entry identity intrinsic.
A reference either resolves to a real entry or, if the target was deleted,
becomes a *detectable* mismatch — never a silent positional aliasing.

### Future features need stable references

Several proposed designs need to point at specific stream entries:
request/response event linking, branching, undo, and plugin event
subscriptions.
Future sub-agent designs that need event-level provenance may also consume event
IDs; [RFD 051] as written works at conversation granularity and does not require
them.
Conversation compaction ([RFD 064]) currently anchors ranges by turn index; once
this RFD lands, event-ID anchors are a candidate replacement, though that
migration is compaction's call, not this RFD's.
That migration also splits along the policy kind, which is worth recording now
so it isn't re-derived later.
A **mechanical** overlay (reasoning/tool-call stripping) carries no derived
content, so by-ID anchoring lets projection apply it to whichever covered events
still exist after a mutation — a robust rebase, no drop needed.
A **summary** overlay carries LLM-generated text describing the events in its
range; removing any covered event makes that text stale, and prose can't be
clipped, so by-ID anchoring buys *precise staleness detection* (the covered-ID
set is no longer fully present) but not a content fix.
The only sound responses to a stale summary are drop or regenerate, and
regeneration is effectful (provider, model config, async), so it must live in
the imperative shell, never in the pure projection layer.
Until that migration lands, an event-removing transformation preserves
compaction overlays whose ranges lie entirely before the earliest removed turn
and drops overlays whose positional anchors may have shifted or lost covered
content (see `ConversationStream::retain`); by-ID anchoring later refines the
drop side of that rule for mechanical overlays.

Without stable IDs, each of these features either reinvents positional
references and inherits the same mutation hazards, or builds a parallel ID
scheme of its own.
The cheapest place to fix this is once, at the stream-entry level.

## Design

### What users see

Nothing changes for normal CLI usage.

For users who edit `events.json` by hand:

- Each stream entry has a short opaque `event_id` field.
- Editing an entry's content keeps its ID; existing references stay valid.
- Duplicating an entry produces two entries with the same ID.
  Load-time repair restores uniqueness in memory by regenerating the ID on the
  later occurrence; the repaired IDs are persisted on the next save.
  A reference that pointed at the duplicated ID is now ambiguous, and reference
  resolution treats it as unresolved rather than silently binding to one of the
  copies.
- Deleting an entry invalidates references to it.
  Downstream features drop the dependent reference.

The `event_id` field is documented in `jp conversation edit`'s help text, with a
note that copies and references behave as described.

### What the data model looks like

The stream-entry identifier lives on the wrapper that holds every entry, not on
each event type.
`InternalEvent` becomes a struct with the `event_id` and a flattened payload
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

Modeling the ID on the wrapper, rather than as a field on each event type,
guarantees at the type level that every stream entry has one.
A new payload variant cannot be added without an ID, and the `Compaction`
overlay is covered for free.
The point of stable IDs is that *any* item in the event array is addressable by
a stable ID instead of by position, whether or not a consumer needs that ID
today.

Programmatic exposure is narrower than addressability: the existing
conversation-event iterators expose `event_id` for the `ConversationEvent`s they
already yield.
Config deltas, compactions, and unknown entries have IDs but gain no new
programmatic surface; the persisted JSON is where every entry is addressable by
ID until a consumer RFD defines the raw-entry view it needs.

`Unknown` keeps its existing forward-compatibility contract: entries with an
unrecognized `type` tag stay invisible to event iteration, config resolution,
and providers, and they participate in uniqueness repair like every other entry.
Because `Unknown` retains the raw JSON object verbatim, deserialization extracts
`event_id` from that object into the wrapper and stores the remaining payload
without it, so serialization writes exactly one `event_id` and the entry still
round-trips losslessly.

The `timestamp` stays on each payload variant.
`event_id` is identity the stream assigns at insertion; `timestamp` is reported
by whoever created the event, and synthetic events deliberately preserve a
source timestamp.
Their provenance differs, so the wrapper owns identity while each payload owns
its own time.

`event_id` and the flattened payload serialize into a single JSON object, so the
on-disk shape is unchanged except for the added `event_id` key:

```json
{ "event_id": "k3m9x2a", "type": "...", "timestamp": "...", ... }
```

The persisted JSON key is `event_id` rather than `id`.
Several event kinds (`ToolCallRequest`, `ToolCallResponse`, `InquiryRequest`,
`InquiryResponse`) already serialize a top-level `id` carrying their own
meaning, and a `ConversationEvent` flattens its `kind` into the same object.
Using `event_id` for the stream-entry identifier keeps both fields side by side
without collision and keeps older readers tolerant: `event_id` is genuinely
unknown to legacy code.

`EventId` is a small opaque newtype in `jp_conversation`, serialized
transparently as a string.
It is strict: deserialization accepts a non-empty string and rejects an empty
one.
The lowercase-alphanumeric form is a *generation* convention, not a *parsing*
constraint, so hand-edited IDs that don't match the format are kept as-is as
long as they're non-empty.
Leniency for a missing or empty ID lives at the storage boundary, not in
`EventId` itself: the stream-entry deserialization treats a missing or empty
`event_id` as "assign a fresh ID at load," matching the existing `InternalEvent`
/ `deserialize_config_delta` compat path.

`EventId` is **not** a `jp_id::Id`.
"Internal" here means: event IDs are not part of JP's user-facing CLI
input/display contract.
Users do not type them as command arguments, JP does not print them outside
raw-JSON contexts, and they do not share the `jp_id` format contract carried by
`ConversationId` and `ProviderId`.
They are visible in `events.json` and, by [RFD 072]'s design, in the
command-plugin protocol.
Adding `event_id` extends that surface the same way any new event field would.
One interaction is worth naming: a legacy conversation that has never been
re-saved gets fresh random IDs at each load (see Drawbacks), so a read-only
plugin can observe different `event_id` values for the same legacy entry across
invocations until the stream is persisted.
`event_id` is host-owned: a future write API (such as RFD 072's `push_events`)
appends payloads and the stream assigns IDs at insertion, so a supplied
`event_id` on a new event is ignored.
The stability contract for that exposure is governed by RFD 072, not by this
RFD.

The format is intentionally short: 7 lowercase alphanumeric characters.
That matches Git's short-ref ergonomic and gives ~78 billion values from a
base-36 alphabet, far more than enough at the ~10k-entry upper bound, especially
with uniqueness enforced at insertion and deterministic load-time repair on
collision (see [Storage-layer repair](#storage-layer-repair) below).

The format is internal and unstable.
Nothing in the public surface relies on the ID's character count, alphabet, or
generation method, and code elsewhere in JP MUST NOT use the ID for ordering,
content addressing, or any property beyond identity-within-a-stream.

### How IDs are assigned

The stream assigns the `event_id` when it wraps a payload into an
`InternalEvent`, which is the one place that has every existing ID in scope.
The event constructors are unchanged: `ConversationEvent::new(kind, ts)`,
`ConversationEvent::now(kind)`, and the `ConfigDelta` constructors keep their
current signatures and gain no ID argument.

The mutation entry points that append to the stream generate the ID:

```rust
impl ConversationStream {
    fn wrap(&self, payload: EventPayload) -> InternalEvent {
        InternalEvent { event_id: self.fresh_event_id(), payload }
    }

    /// A random `EventId` that does not collide with any entry already in the
    /// stream.
    /// The stream knows the existing IDs, so this is where "unique within its
    /// stream" is enforced.
    fn fresh_event_id(&self) -> EventId { /* retry random() until unused */ }
}
```

The invariant: every path that creates or inserts an `InternalEvent` goes
through this wrap constructor; deserialization is the only path that preserves
an existing ID.
The known call sites are `push`, `add_config_delta`, `add_compaction`, `extend`,
`TurnMut::build`, `start_turn`, the synthetic insertions in
`normalize_turn_starts` and `sanitize_orphaned_tool_calls`, and projection's
injected entries (ephemeral; see [Projection views](#projection-views)).
Because generation happens where stream context exists, "unique within its
stream" holds at insertion, not merely after a load-time pass.
A fixture or test that needs a deterministic ID constructs the `InternalEvent`
wrapper directly with a fixed `EventId`; live and synthetic insertion paths let
the stream assign one.
There is no thread-local RNG and no test/prod plumbing inside the event
constructors.

Synthetic stream entries that must keep a specific timestamp (for example, a
`TurnStart` that adopts the first chat request's time, or a synthetic
`ToolCallResponse` that adopts its original request's time) keep setting it on
the payload as they do today.
Only the ID is stream-assigned.
Silently shifting those timestamps to "now" would change ordering semantics.

**Loaded entries with an ID present.** Kept as-is.

**Loaded entries without an ID.** A fresh random `EventId` is assigned at load
time and held in memory.
Re-saving the stream persists the assigned IDs; from then on they behave like
any other ID.

### Projection views

`event_id` stability is a property of the persisted raw stream.
Compaction projection builds an ephemeral provider view by transforming a copy
of the stream, injecting synthetic entries (for example the summary
`ChatRequest` / `ChatResponse` pair) that exist only in that view.
Those entries are wrapped like any other and receive fresh IDs, but the IDs are
ephemeral: they carry no stability contract across projections, are never
persisted or exposed through storage or plugin APIs, and MUST NOT be used as
references into `events.json`.

### Dependency

`EventId::random()` requires a source of randomness, which the workspace does
not currently depend on directly.
We promote `getrandom` (already present transitively through several paths) to a
direct workspace dependency and use it from `jp_conversation`.

`getrandom` is the lowest-friction choice: a thin wrapper around the OS RNG, no
distribution machinery, no allocator dependencies.
Heavier alternatives (`rand`, `uuid`, `ulid`, `nanoid`) are unnecessary; their
additional surface area buys properties (distributions, time-sortability,
universal uniqueness) that have no consumer here.

OS RNG failure is treated as fatal: `EventId::random()` panics with a clear
message rather than returning a `Result`.
`getrandom` fails only when the OS entropy source is broken, a state in which
far more than JP is unusable, and threading a `Result` through every append API
(including `Extend`, which cannot return one) would poison the call graph for an
unrecoverable condition.
The append APIs stay infallible.

### Storage-layer repair

ID-uniqueness repair runs inside `ConversationStream::from_parts` and
`from_legacy_events`, immediately after deserialization, before the stream is
returned.
It is **not** part of `ConversationStream::sanitize()`, which is reserved for
higher-level stream mutations (orphaned tool responses, orphaned inquiry
responses, leading non-user events, turn-start normalization).

```txt
collect ids; for each duplicate id, regenerate it on the later occurrence(s)
so the stream again has unique ids; record which ids were duplicated.
```

Repair restores the *uniqueness* invariant, but it cannot restore reference
*intent*.
A manual edit that duplicates an ID (a copy-paste, or a reorder that moves a
copy ahead of the original) makes any reference to that ID inherently ambiguous:
there is no way to know which occurrence a pre-existing reference meant.
Regenerating the later occurrence restores uniqueness in memory (persisted on
the next save), but it does not make a stale reference correct, and keeping the
earliest occurrence's ID does **not** by itself preserve reference validity in
the reorder case.

So a duplicated ID is treated as an ambiguous condition, not merely a missing
one.
Reference resolution (in the future overlay features that consume IDs) treats a
reference to an ID that was duplicated at load as **unresolved**, the same as a
reference to a deleted ID.
This is what makes the Motivation's claim hold: a copy, reorder, or delete
produces a *detectable* mismatch, never a silent positional rebind to the wrong
entry.

The set of duplicated IDs is retained as private, load-scoped state on
`ConversationStream`, populated by the repair pass and consulted by future
reference resolution to classify a reference as ambiguous.
Once the repaired stream is saved and reloaded, the file has unique IDs and the
set is empty.
No public accessor is added until a consumer exists, consistent with the
`event_ids()` stance below.

RFDs that introduce reference-bearing entries must consume this recorded
ambiguity in the same load cycle, before the stream is persisted: resolve or
drop ambiguous references prior to any save, or refuse the write.
Persisting a repaired stream without that cleanup discards the ambiguity signal,
and a stale reference to the surviving ID would resolve silently on the next
load.

Repair logs a warning per regenerated ID but returns no caller-facing report.
Surfacing repair to the user or to overlays is out of scope for this RFD; future
overlay-specific RFDs that anchor by ID define their own surfacing for ambiguous
and orphaned references.

Existing `sanitize` steps continue to work unchanged.
Future overlay types that reference event IDs add a "drop dependent overlay"
step over the IDs of *all* stream entries (every `InternalEvent`, regardless of
payload kind):

```txt
if overlay.anchor_id was duplicated at load, or
   overlay.anchor_id ∉ { id of every stream entry } { drop overlay }
```

This RFD does not add a public `event_ids()` accessor; there is no consumer yet.
The ID set is an internal notion the repair pass already computes, and the
public API can grow an accessor when a real consumer (compaction, future
sub-agent provenance features, plugin subscriptions, interactive editing)
defines the shape it needs.

### Storage

`events.json` gains a short `event_id` key per stream entry.
The on-disk growth is negligible even for the largest conversations.

The on-disk format is forward-compatible: older readers ignore the unknown
`event_id` field on stream entries.
Backward-compatible because legacy files without `event_id` get random IDs
assigned at load and persisted on the next save.

## Drawbacks

- **The `InternalEvent` wrapper is a one-time structural change.**
  `InternalEvent` becomes a struct with a flattened payload enum, and its
  hand-written `Serialize` / `Deserialize` get rewritten to that shape while
  preserving the `type` tag and the base64 encode/decode hooks.
  The iteration views expose `event_id`.
  Because the ID lives on the wrapper and is assigned at insertion, the event
  constructors are unchanged, so the hundreds of `ConversationEvent::new` /
  `ConfigDelta` call sites in tests do **not** need an explicit ID; fixtures
  that assert on a specific ID construct the wrapper directly.
  The serialized form is unchanged except for the added key, so the change is
  mechanical.
  The fiddly part is the custom serde, worth spiking first to confirm a
  byte-for-byte round-trip against existing fixtures.
- **Storage load gains a new pass.** Uniqueness enforcement runs on every load
  via `from_parts` / `from_legacy_events`.
  Cheap, but it is an added load-time pass.
- **Manual-edit footgun for the duplicate case.** A user who copy-pastes an
  entry sees both copies in the file, but on the next load one of them gets a
  new ID, and any reference to the duplicated ID becomes unresolved.
  This is documented but not visible until reload.
- **In-memory IDs for legacy entries are not stable across loads.** Loading a
  legacy file twice without saving in between produces two different random ID
  sets.
  Acceptable today because nothing consumes those IDs in the unsaved state; it
  would become a problem only if a future feature took a hard dependency on
  cross-load ID stability without first persisting.
- **`timestamp` stays duplicated across payload variants.** The wrapper holds
  `event_id`, so the identifier is declared once.
  `timestamp`, by contrast, stays on each payload variant rather than moving to
  the wrapper.
  Lifting it would buy no new type-level guarantee (every variant already
  carries a timestamp) and would re-introduce churn at every event constructor
  and timestamp read site, the very churn the wrapper avoids for `event_id`.
  The provenance also differs: `event_id` is identity the stream assigns, while
  `timestamp` is reported by the event's producer.
  Keeping `timestamp` on the payload is deliberate; revisit only if a third
  field genuinely shared across all entries forces the wrapper to grow.

## Alternatives

**1. Keep position-based references; drop dependent overlays on mutation.**
Localizes the cost — each feature that needs to anchor to entries handles
mutation locally — but doesn't generalize.
Every future feature reinvents the workaround, and each implementation is a
fresh chance to get it wrong.

**2. Anchor only `TurnStart` events.** Smaller schema change, but every later
feature that needs to reference a non-turn entry has to add IDs to its
referenced types.
Punts the problem.

**3. Content-addressed IDs (hash of the entry's serialized content).**
Cryptographically clean, no random generation needed.
But editing content invalidates the ID, which breaks the manual-edit use case
that motivates this RFD.

**4. Longer or structured ID formats (UUIDv4, UUIDv7, ULID, `jp_id`
membership).** All over-specified for a per-conversation, internal, opaque
identifier.
UUIDs and ULIDs add bytes for properties — universal uniqueness,
time-sortability — that have no consumer here.
Stream order is the array, not the ID.
`jp_id` membership leaks an internal storage detail into the public ID format
contract.

**5. Generate the ID inside the event constructor (explicit argument or
thread-local RNG).** An earlier shape put `event_id` on `ConversationEvent` /
`ConfigDelta` and had the constructor supply it, either via an explicit argument
(churn at every call site) or a thread-local RNG (non-determinism inside
constructors, plus test plumbing to tame it).
Assigning the ID on the `InternalEvent` wrapper at insertion is better than
both: the constructors stay unchanged, there is no hidden RNG state, and
generation happens where the stream's existing IDs are in scope, so uniqueness
is enforceable at insertion.

**6. Also lift `timestamp` onto the `InternalEvent` wrapper.** This design puts
`event_id` on the wrapper; the open question is whether `timestamp` should move
there too.
It is not worth it now.
With `event_id` on the wrapper, `timestamp` is the only field still repeated
across variants, which is the N=1 situation that did not justify a wrapper
before.
Moving it adds no type-level guarantee (every variant already has a timestamp)
and re-introduces constructor and read-site churn.
The end state where both live on the wrapper may be right eventually; revisit
when a concrete third shared field forces it.

## Non-Goals

- **Cryptographic identity.** IDs are not tamper-evident.
  Nothing depends on them being unforgeable.
- **Universal uniqueness.** Required uniqueness is *within a single
  conversation's stream*.
  Cross-conversation, cross-workspace, and cross-machine collisions are out of
  scope.
- **Time-sortability.** Stream order is the array order.
  IDs do not encode time, and code MUST NOT use them for ordering.
- **Stable identity for projected views.** Projection-created synthetic entries
  receive fresh ephemeral IDs with no stability contract; nothing may reference
  them (see [Projection views](#projection-views)).
- **Content-addressing or deduplication.** Two entries with identical content
  get distinct IDs.
  [RFD 067] covers deduplication separately.
- **Streaming or replication semantics.** This RFD is local to a single
  conversation file.
- **Surfacing repair to users or callers.** Load-time ID-repair logs a warning
  but does not return a structured report.
  Per-overlay surfacing for orphaned references is the responsibility of
  overlay-specific RFDs.
- **Refactoring existing reference sites.** Future features that want to use
  event IDs (compaction, future sub-agent provenance features, plugin event
  subscriptions, interactive stream editing) migrate their reference layout
  themselves.
- **Plugin-visible event-identity semantics beyond JSON exposure.** RFD 072
  governs what plugins see in `read_events`; this RFD just adds a field to the
  same JSON format.

## Risks and Open Questions

- **Repair cost on load.** ID-uniqueness repair adds a uniqueness pass over
  stream entries on every load.
  Cheap but revisit if profiling shows it.
- **Fixture migration scope.** The event constructors are unchanged, so most
  test call sites are untouched.
  The churn is concentrated in the `InternalEvent` serde rewrite, the
  iteration-view change, and the snapshots that gain an `event_id` key.
  Fixtures that assert on a specific ID construct the wrapper directly; a short
  helper for readable fixed IDs keeps those legible.

## Implementation Plan

### Phase 1 — `EventId` type and dependency

- Promote `getrandom` to a direct workspace dependency.
- Add `EventId` newtype in `jp_conversation`.
  Internal opaque ID, no `jp_id` membership.
- Implement `EventId::random()` on top of `getrandom`, transparent string serde,
  `Display`/`Debug`, and a test-fixture helper for readable fixed IDs.
- Tests: serde round-trip; `EventId` deserialization rejects an empty string
  (strict); non-format but non-empty strings are preserved as-is; two `random()`
  calls produce distinct IDs.

Independent.
Mergeable on its own.

### Phase 2 — `InternalEvent` wrapper and stream-assigned IDs

- Make `InternalEvent` a struct: `event_id: EventId` plus a flattened
  `EventPayload` enum (`Event` / `ConfigDelta` / `Compaction` / `Unknown`).
- Rewrite `InternalEvent`'s `Serialize` / `Deserialize` to the struct+flatten
  shape, preserving the `type` tag, the base64 encode/decode hooks, and the
  lossless round-trip of `Unknown` entries (extract `event_id` from the raw
  object on load; write exactly one `event_id` on save).
  Confirm a byte-for-byte round-trip against existing fixtures, modulo the new
  key.
- Assign `event_id` on insertion: route every `InternalEvent` creation through
  the wrap constructor (`push`, `add_config_delta`, `add_compaction`, `extend`,
  `TurnMut::build`, `start_turn`, and the synthetic insertions in
  `normalize_turn_starts` / `sanitize_orphaned_tool_calls`), backed by a
  `fresh_event_id()` that avoids collision with IDs already in the stream.
- Expose `event_id` on the iteration views, for the `ConversationEvent`s they
  already yield; config deltas, compactions, and unknown entries gain no new
  programmatic surface (they remain ID-addressable in the persisted JSON).
- On deserialize, treat a missing or empty `event_id` as "assign a fresh ID at
  load"; `EventId` itself stays strict.
- Regenerate the snapshots and fixtures that gain an `event_id` key in this same
  change, so the phase lands green.
- Verify no test relies on positional references that stable IDs would break.

Depends on Phase 1.
Touches the stream core and the iteration views; the serde rewrite is the fiddly
part.

### Phase 3 — Storage-layer uniqueness repair

- Add `ensure_unique_event_ids` in `ConversationStream`, recording which IDs
  were duplicated in a private, load-scoped field on the stream (no public
  accessor until a consumer exists).
- Call it from `from_parts` and `from_legacy_events` after deserialization,
  before the stream is returned.
  **Not** part of `sanitize()`.
- Test: a stream with two entries sharing an explicit fixed ID; repair
  regenerates the later occurrence and the in-memory stream has unique IDs.
- Test: a legacy file with no `event_id` fields; entries get IDs assigned at
  load.
- Test: a live insertion path never exposes a duplicate ID; force a collision in
  `fresh_event_id` and verify it retries.

Depends on Phase 2.

### Phase 4 — Documentation

- Update `jp conversation edit --events` help text with the duplicate-ID note
  (using `event_id`): a copy gets a new ID on the next load, and a reference to
  a duplicated ID becomes unresolved.
- Update the data model section of `docs/architecture/` if applicable.
- Glossary entry in `docs/architecture/ubiquitous-language.md` for "Event ID."

Can land alongside Phase 3.

Future RFDs that want to reference entries by ID (compaction, future sub-agent
provenance features, plugin event subscriptions, interactive stream editing)
migrate their reference layout in their own implementation work.
That is out of scope here.

## References

- [RFD 047] — Editor and Path Access for Conversations (motivates manual
  editing of `events.json`)
- [RFD 051] — Sub-Agent Workflows (conversation-level as written; not an
  event-ID consumer)
- [RFD 054] — Split Conversation Config and Events (storage shape this RFD
  modifies)
- [RFD 064] — Non-Destructive Conversation Compaction (potential future
  consumer; currently anchors by turn index)
- [RFD 067] — Resource Deduplication (related but distinct concern)
- [RFD 072] — Command Plugin System (governs plugin-protocol exposure of
  `ConversationEvent` JSON)

[RFD 047]: 047-editor-and-path-access-for-conversations.md
[RFD 051]: 051-sub-agent-workflows.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[RFD 072]: 072-command-plugin-system.md
