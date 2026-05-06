# RFD D20: Stable Event Identifiers

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-03

## Summary

Every conversation event carries a stable identifier, unique within its
stream. Events and overlays reference each other by ID rather than by
position. Loaded events without an ID get a fresh random ID assigned at
load time and persisted on the next save.

## Motivation

This is a technical change. It does not add a user-facing feature today; it
removes a class of correctness problem and unblocks several future features.

### Manual edits are unsafe today

JP encourages users to edit `events.json` by hand via `jp conversation edit
--events`. In practice, that workflow has three common shapes:

- **Delete a digression.** Let a tangent play out for a few turns, decide it
  didn't go anywhere useful, and remove those events.
- **Rewind an in-flight bad query.** Cancel a request mid-stream, drop the
  partial response, edit the user message in the JSON, and restart with `jp q
  --no-edit`.
- **Drop noisy tool calls.** Remove tool requests/responses whose output is
  polluting the working context going forward.

Each of these is a structural mutation. Position-based references between events
are silently invalidated by exactly this kind of edit:

- Reordering events changes what "turn 3" means without any signal.
- Copying an event preserves its content but breaks identity-by-position.
- Deleting an event silently invalidates every reference into that range.

Stable IDs make event identity intrinsic. References either resolve to a real
event or fail loudly during sanitize.

### Future features need stable references

Several proposed designs need to point at specific events, for example:
sub-agent workflows ([RFD 051]), branching, undo, plugin, and conversation
compaction ([RFD 064]). Without stable IDs, each of them either reinvents
positional references and inherits the same mutation hazards, or builds a
parallel ID scheme of its own.

The cheapest place to fix this is once, at the event level.

## Design

### What users see

Nothing changes for normal CLI usage.

For users who edit `events.json` by hand:

- Each event has a short opaque `id` field.
- Editing an event's content keeps its ID; existing references stay valid.
- Duplicating an event produces two events with the same ID. Sanitize detects
  this on the next load and regenerates the ID on the later occurrence.
- Deleting an event invalidates references to it. Downstream features drop the
  dependent event reference during sanitize.

The `id` field is documented in `jp conversation edit`'s help text, with a note
that copies and references behave as described.

### What the data model looks like

`ConversationEvent` gains an `id` field alongside `timestamp` and `kind`:

```rust
pub struct ConversationEvent {
    pub id: EventId,
    pub timestamp: DateTime<Utc>,
    pub kind: EventKind,
}
```

`EventId` is a small opaque newtype in `jp_conversation`, serialized as a short
lowercase-alphanumeric string of around 6–8 characters.

It is **not** a `jp_id::Id`. Event IDs are storage internals — users never type
them at the CLI, and they don't share the public ID format contract with
`ConversationId` or `ProviderId`.

The format is intentionally short. A conversation stream is rarely above ~2000
events; even 6 characters is more than enough to make random collisions rare
inside a single stream. Sanitize handles any collision case deterministically
(see below).

The format is internal and unstable. Nothing in the public surface relies on the
ID's character count, alphabet, or generation method, and code elsewhere in JP
MUST NOT use the ID for ordering, content addressing, or any property beyond
identity-within-a-stream.

### How IDs are assigned

`ConversationEvent::new` takes the `EventId` explicitly:

```rust
impl ConversationEvent {
    pub fn new(id: EventId, kind: impl Into<EventKind>, ts: impl Into<DateTime<Utc>>) -> Self;

    pub fn now(kind: impl Into<EventKind>) -> Self {
        Self::new(EventId::random(), kind, Utc::now())
    }
}
```

`now` is the production helper: it generates a fresh random ID and uses the
current timestamp. `new` requires an explicit ID, which keeps tests
deterministic without thread-local RNG state — fixtures pass a fixed `EventId`
literal and snapshots stay stable across runs.

This costs the most diff at call sites — every `ConversationEvent::new` in the
test suite gets an explicit ID. There is no thread-local RNG, no test/prod
plumbing, no hidden non-determinism inside a constructor.

**Loaded events with an ID present.** Kept as-is.

**Loaded events without an ID.** A fresh random `EventId` is generated at load
time and held in memory. Re-saving the stream persists the assigned IDs; from
then on they behave like any other ID.

### Sanitize

A new sanitize step enforces ID uniqueness:

```txt
collect ids; for each duplicate, regenerate the id on the later occurrence.
```

The earliest occurrence keeps its ID, so existing references remain valid. A
user who copy-pastes an event in the JSON sees both copies in the file, but on
the next load one of them gets a new identity.

Existing sanitize steps (orphaned tool responses, orphaned inquiry responses,
leading non-user events) continue to work. Future overlay types that reference
event IDs add a "drop dependent overlay" step:

```txt
if overlay.anchor_id ∉ stream.event_ids() { drop overlay }
```

### Storage

`events.json` gains an `id` field per event. With 6–8 character IDs, that's
~10–12 bytes per event including the JSON key. A 10,000-event conversation grows
by ~100 KB. Negligible.

The on-disk format is forward-compatible: older readers ignore unknown fields on
`ConversationEvent`. Backward-compatible because legacy files without `id` get
random IDs assigned at load and persisted on the next save.

## Drawbacks

- **Schema change touches every event constructor and many tests.** Every
  `ConversationEvent::new` call site updates to pass an explicit `EventId`.
  Production code uses `ConversationEvent::now`. Tests pass fixed IDs from
  helpers. Mechanical but extensive — hundreds of call sites in
  `jp_cli/src/cmd/conversation/{fork,grep,print}_tests.rs` and across
  `jp_conversation` and `jp_llm`.
- **Sanitize gains a new invariant.** Uniqueness enforcement runs on every load.
  Cheap (`HashSet` over event count) but adds a load-time pass.
- **Manual-edit footgun for the duplicate case.** A user who copy-pastes an
  event sees both copies in the file, but on the next load one of them gets a
  new ID. This is documented but not visible until reload.
- **In-memory IDs for legacy events are not stable across loads.** Loading a
  legacy file twice without saving in between produces two different random ID
  sets. Acceptable today because nothing consumes those IDs in the unsaved
  state; it would become a problem only if a future feature took a hard
  dependency on cross-load ID stability without first persisting.

## Alternatives

**1. Keep position-based references; drop dependent overlays on mutation.**
Localizes the cost — each feature that needs to anchor to events handles
mutation locally — but doesn't generalize. Every future feature reinvents the
workaround, and each implementation is a fresh chance to get it wrong.

**2. Anchor only TurnStart events.** Smaller schema change, but every later
feature that needs to reference a non-turn event has to add IDs to its
referenced types. Punts the problem.

**3. Content-addressed IDs (hash of the event's serialized content).**
Cryptographically clean, no random generation needed. But editing content
invalidates the ID, which breaks the manual-edit use case that motivates this
RFD.

**4. Longer or structured ID formats (UUIDv4, UUIDv7, ULID, `jp_id`
membership).** All over-specified for a per-conversation, internal,
opaque identifier. UUIDs and ULIDs add bytes for properties — universal
uniqueness, time-sortability — that have no consumer here. Stream order
is the array, not the ID. `jp_id` membership leaks an internal storage
detail into the public ID format contract.

**5. Auto-generate the ID inside `new` via a thread-local RNG.** Smaller
diff than passing the ID explicitly, but introduces non-determinism into
`ConversationEvent::new`. The "make tests deterministic" plumbing offsets
the diff savings and hides the cost rather than removing it.

## Non-Goals

- **Cryptographic identity.** IDs are not tamper-evident. Nothing depends
  on them being unforgeable.
- **Universal uniqueness.** Required uniqueness is *within a single
  conversation's event stream*. Cross-conversation, cross-workspace, and
  cross-machine collisions are out of scope.
- **Time-sortability.** Stream order is the array order. IDs do not encode
  time, and code MUST NOT use them for ordering.
- **Content-addressing or deduplication.** Two events with identical
  content get distinct IDs. RFD 067 covers deduplication separately.
- **Streaming or replication semantics.** This RFD is local to a single
  conversation file.
- **Refactoring existing reference sites.** Future features that want to
  use event IDs (compaction, sub-agent workflows, plugin event
  subscriptions, interactive stream editing) migrate their reference
  layout themselves.
- **Plugin-visible event identity.** A separate RFD covers plugin event
  subscriptions; this RFD just provides the underlying primitive.

## Risks and Open Questions

- **Eager vs lazy sanitize.** Default to eager: enforce uniqueness on
  every load. Cost is `HashSet` over event count; revisit only if it
  shows up in profiles.
- **Test fixture migration scope.** Hundreds of `ConversationEvent::new`
  call sites in tests need an explicit ID argument. The migration is
  mechanical (assign a fixed ID per call), but it is the bulk of the
  diff. A short helper for generating readable fixture IDs keeps call
  sites legible.
- **Storage upper bound.** 10k-event conversation grows by ~100 KB.
  Acceptable.

## Implementation Plan

### Phase 1 — `EventId` type

- Add `EventId` newtype in `jp_conversation`. Internal opaque ID, no
  `jp_id` membership.
- Implement `EventId::random()`, serde impls, `Display`/`Debug`, and a
  test-fixture helper for readable fixed IDs.
- Tests: serde round-trip, two `random()` calls produce distinct IDs.

Independent. Mergeable on its own.

### Phase 2 — `ConversationEvent::id`

- Add `id: EventId` to `ConversationEvent`.
- Update `new(id, kind, ts)` and `now(kind)`. `now` calls
  `EventId::random()`; `new` takes the ID explicitly.
- Update serde: serialize `id`; on deserialize, accept a missing field and
  generate a fresh `random()` ID.
- Update the `compat` deserializer to populate IDs for legacy events.
- Migrate every test fixture in the workspace to pass an explicit
  `EventId`.

Depends on Phase 1. Touches many files but mechanically.

### Phase 3 — Sanitize uniqueness

- Add `enforce_event_id_uniqueness` to `ConversationStream::sanitize`.
- Test: stream with duplicate IDs — later occurrence gets a new ID,
  earlier occurrence keeps its ID.
- Test: forced collision between random IDs gets resolved deterministically.

Depends on Phase 2.

### Phase 4 — Snapshot regeneration

- Regenerate any snapshot fixtures affected by the new `id` field across
  `jp_conversation`, `jp_llm/tests/fixtures/**`, and `jp_md`.
- Verify no test relies on positional references that IDs would break.

Depends on Phases 1–3.

### Phase 5 — Documentation

- Update `jp conversation edit --events` help text with the
  duplicate-ID note.
- Update the data model section of `docs/architecture/` if applicable.
- Glossary entry in `docs/architecture/ubiquitous-language.md` for
  "Event ID."

Independent of the others, can land alongside Phase 4.

Future RFDs that want to reference events by ID — compaction, sub-agents,
plugin event subscriptions, interactive stream editing — migrate their
reference layout in their own implementation work. That is out of scope
here.

## References

- [RFD 051] — Sub-Agent Workflows (future consumer)
- [RFD 064] — Non-Destructive Conversation Compaction (future consumer)
- [RFD 067] — Resource Deduplication (related but distinct concern)
- [RFD D18] — Plugin Event Subscriptions (future consumer)
- [RFD D21] — Interactive Conversation Stream Editing (future consumer)

[RFD 051]: 051-sub-agent-workflows.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[RFD D18]: drafts/D18-plugin-event-subscriptions-and-query-delegation.md
[RFD D21]: drafts/D21-interactive-conversation-stream-editing.md
