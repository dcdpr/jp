# RFD D24: Stable Event Identifiers

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-03

## Summary

Every entry in a conversation's event stream — both `ConversationEvent`s and
`ConfigDelta`s — carries a stable identifier, unique within its stream.
Stream entries and overlays reference each other by ID rather than by position.
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

Several proposed designs need to point at specific stream entries: sub-agent
workflows ([RFD 051]), branching, undo, and plugin event subscriptions ([RFD
D18]).
Conversation compaction ([RFD 064]) currently anchors ranges by turn index; once
D24 lands, event-ID anchors are a candidate replacement, though that migration
is compaction's call, not D24's.

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
  Storage's load-time repair detects this and regenerates the ID on the later
  occurrence.
- Deleting an entry invalidates references to it.
  Downstream features drop the dependent reference.

The `event_id` field is documented in `jp conversation edit`'s help text, with a
note that copies and references behave as described.

### What the data model looks like

`ConversationEvent` and `ConfigDelta` each gain an `event_id` field alongside
`timestamp`:

```rust
pub struct ConversationEvent {
    pub event_id: EventId,
    pub timestamp: DateTime<Utc>,
    pub kind: EventKind,
    pub metadata: Map<String, Value>,
}

pub struct ConfigDelta {
    pub event_id: EventId,
    pub timestamp: DateTime<Utc>,
    pub delta: Box<PartialAppConfig>,
}
```

The persisted JSON key is `event_id` rather than `id`.
`ConversationEvent` flattens its `kind` into the same JSON object, and several
event kinds (`ToolCallRequest`, `ToolCallResponse`, `InquiryRequest`,
`InquiryResponse`) already serialize a top-level `id` carrying their own
meaning.
Using `event_id` for the stream-entry identifier keeps both fields side by side
without collision and keeps older readers tolerant — `event_id` is genuinely
unknown to legacy code.

`EventId` is a small opaque newtype in `jp_conversation`, serialized
transparently as a string.
It is a thin wrapper over `String`: deserialization accepts any non-empty
string.
The lowercase-alphanumeric form is a *generation* convention, not a *parsing*
constraint — hand-edited IDs that don't match the format are kept as-is, as
long as they're non-empty and unique.
Empty strings are treated as missing IDs and regenerated at load.

`EventId` is **not** a `jp_id::Id`.
"Internal" here means: event IDs are not part of JP's user-facing CLI
input/display contract.
Users do not type them as command arguments, JP does not print them outside
raw-JSON contexts, and they do not share the `jp_id` format contract carried by
`ConversationId` and `ProviderId`.
They are visible in `events.json` and, by [RFD 072]'s design, in the
command-plugin protocol — adding `event_id` extends that surface in the same
way any new event field would, and the stability of that exposure is governed by
RFD 072, not by this RFD.

The format is intentionally short: 7 lowercase alphanumeric characters.
That matches Git's short-ref ergonomic and gives ~78 billion values from a
base-36 alphabet — far more than enough at the ~10k-entry upper bound,
especially with deterministic load-time repair on collision (see [Storage-layer
repair](#storage-layer-repair) below).

The format is internal and unstable.
Nothing in the public surface relies on the ID's character count, alphabet, or
generation method, and code elsewhere in JP MUST NOT use the ID for ordering,
content addressing, or any property beyond identity-within-a-stream.

### How IDs are assigned

Both `ConversationEvent` and `ConfigDelta` expose a three-way constructor
pattern:

```rust
impl ConversationEvent {
    /// Explicit ID, explicit timestamp. Used by tests and fixtures.
    pub fn new(id: EventId, kind: impl Into<EventKind>, ts: impl Into<DateTime<Utc>>) -> Self;

    /// Random ID, explicit timestamp. Used by synthetic-event sites that
    /// must preserve a related event's timestamp.
    pub fn at(kind: impl Into<EventKind>, ts: impl Into<DateTime<Utc>>) -> Self {
        Self::new(EventId::random(), kind, ts)
    }

    /// Random ID, current timestamp. Used by live event creation.
    pub fn now(kind: impl Into<EventKind>) -> Self {
        Self::at(kind, Utc::now())
    }
}
```

`ConfigDelta` follows the same shape (`new(id, delta, ts)` / `at(delta, ts)` /
`now(delta)`).

`at` is the production helper for synthetic stream entries that must keep a
specific timestamp — for example, the `TurnStart` inserted by
`normalize_turn_starts` (which uses the timestamp of the first chat request) or
the synthetic `ToolCallResponse` inserted by `sanitize_orphaned_tool_calls`
(which uses the original request's timestamp).
Silently shifting these to "now" would change ordering semantics.

`new` requires an explicit ID, which keeps tests deterministic without
thread-local RNG state — fixtures pass a fixed `EventId` literal and snapshots
stay stable across runs.

This costs the most diff at call sites — every existing
`ConversationEvent::new` and `ConfigDelta` constructor in the test suite gets an
explicit ID.
There is no thread-local RNG, no test/prod plumbing, no hidden non-determinism
inside a constructor.

**Loaded entries with an ID present.** Kept as-is.

**Loaded entries without an ID.** A fresh random `EventId` is generated at load
time and held in memory.
Re-saving the stream persists the assigned IDs; from then on they behave like
any other ID.

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

### Storage-layer repair

ID-uniqueness repair runs inside `ConversationStream::from_parts` and
`from_legacy_events`, immediately after deserialization, before the stream is
returned.
It is **not** part of `ConversationStream::sanitize()`, which is reserved for
higher-level stream mutations (orphaned tool responses, orphaned inquiry
responses, leading non-user events, turn-start normalization).

```txt
collect ids; for each duplicate, regenerate the id on the later occurrence.
```

The earliest occurrence keeps its ID, so existing references remain valid.
A user who copy-pastes an entry in the JSON sees both copies in the file, but on
the next load one of them gets a new identity.

Repair logs a warning per regenerated ID but does not return a structured
report.
Surfacing repair to the user or to overlays is out of scope for this RFD; future
overlay-specific RFDs that anchor by ID will define their own surfacing for
orphaned references when needed.

Existing `sanitize` steps continue to work unchanged.
Future overlay types that reference event IDs add a "drop dependent overlay"
step:

```txt
if overlay.anchor_id ∉ stream.event_ids() { drop overlay }
```

Per the Motivation, this is a *detectable* mismatch — the anchor either
resolves or it doesn't — not a silent positional aliasing.

### Storage

`events.json` gains an `event_id` field per stream entry.
With 7-character IDs, that's ~12 bytes per entry including the JSON key. A
10,000-entry conversation grows by ~120 KB.
Negligible.

The on-disk format is forward-compatible: older readers ignore the unknown
`event_id` field on stream entries.
Backward-compatible because legacy files without `event_id` get random IDs
assigned at load and persisted on the next save.

## Drawbacks

- **Schema change touches every stream-entry constructor and many tests.** Every
  existing `ConversationEvent::new` and `ConfigDelta` constructor call site
  updates to pass an explicit `EventId`, or migrates to `at` / `now`.
  Production code uses `at` or `now`; tests pass fixed IDs from helpers.
  Mechanical but extensive — hundreds of call sites in
  `jp_cli/src/cmd/conversation/{fork,grep,print}_tests.rs` and across
  `jp_conversation` and `jp_llm`.
- **Storage load gains a new pass.** Uniqueness enforcement runs on every load
  via `from_parts` / `from_legacy_events`.
  Cheap (`HashSet` over entry count) but adds a load-time pass.
- **Manual-edit footgun for the duplicate case.** A user who copy-pastes an
  entry sees both copies in the file, but on the next load one of them gets a
  new ID.
  This is documented but not visible until reload.
- **In-memory IDs for legacy entries are not stable across loads.** Loading a
  legacy file twice without saving in between produces two different random ID
  sets.
  Acceptable today because nothing consumes those IDs in the unsaved state; it
  would become a problem only if a future feature took a hard dependency on
  cross-load ID stability without first persisting.
- **Field duplication between `ConversationEvent` and `ConfigDelta`.** Both
  types now carry `event_id` and `timestamp`.
  The duplication points at a latent shape — both are stream entries with
  shared metadata wrapping a kind-specific payload — but lifting those fields
  into a wrapper struct around `InternalEvent` would require removing
  `timestamp` from `ConversationEvent`'s public type, and the timestamp is
  conceptually inseparable from the event it stamps.
  Living with the duplication is the right call for D24; consolidation is future
  work.

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

**5. Auto-generate the ID inside `new` via a thread-local RNG.** Smaller diff
than passing the ID explicitly, but introduces non-determinism into
constructors.
The "make tests deterministic" plumbing offsets the diff savings and hides the
cost rather than removing it.

**6. Lift `event_id` and `timestamp` into a wrapper around `InternalEvent`.**
Resolves the duplication noted in Drawbacks, but requires removing `timestamp`
from `ConversationEvent` (a widely-consumed public type whose timestamp is part
of its identity).
The rip-up cost dominates the duplication cost.
Re-evaluate if additional `InternalEvent` variants accumulate the same shared
metadata.

## Non-Goals

- **Cryptographic identity.** IDs are not tamper-evident.
  Nothing depends on them being unforgeable.
- **Universal uniqueness.** Required uniqueness is *within a single
  conversation's stream*.
  Cross-conversation, cross-workspace, and cross-machine collisions are out of
  scope.
- **Time-sortability.** Stream order is the array order.
  IDs do not encode time, and code MUST NOT use them for ordering.
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
  event IDs (compaction, sub-agent workflows, plugin event subscriptions,
  interactive stream editing) migrate their reference layout themselves.
- **Plugin-visible event-identity semantics beyond JSON exposure.** RFD 072
  governs what plugins see in `read_events`; this RFD just adds a field to the
  same JSON format.

## Risks and Open Questions

- **Repair cost on load.** ID-uniqueness repair adds a `HashSet` pass over
  stream entries on every load.
  Cheap but revisit if profiling shows it.
- **Test fixture migration scope.** Hundreds of `ConversationEvent` and a
  handful of `ConfigDelta` constructor call sites in tests need explicit ID
  arguments.
  The migration is mechanical (assign a fixed ID per call), but it is the bulk
  of the diff.
  A short helper for generating readable fixture IDs keeps call sites legible.
- **Storage upper bound.** 10k-entry conversation grows by ~120 KB.
  Acceptable.

## Implementation Plan

### Phase 1 — `EventId` type and dependency

- Promote `getrandom` to a direct workspace dependency.
- Add `EventId` newtype in `jp_conversation`.
  Internal opaque ID, no `jp_id` membership.
- Implement `EventId::random()` on top of `getrandom`, transparent string serde,
  `Display`/`Debug`, and a test-fixture helper for readable fixed IDs.
- Tests: serde round-trip; deserialization of empty string treated as missing;
  deserialization of non-format strings preserved as-is; two `random()` calls
  produce distinct IDs.

Independent.
Mergeable on its own.

### Phase 2 — `event_id` on stream entries

- Add `event_id: EventId` to `ConversationEvent` and `ConfigDelta`.
- Update constructors:
  - `ConversationEvent::new(id, kind, ts)`, `ConversationEvent::at(kind, ts)`,
    `ConversationEvent::now(kind)`.
  - `ConfigDelta::new(id, delta, ts)`, `ConfigDelta::at(delta, ts)`,
    `ConfigDelta::now(delta)`.
- Update serde for both: serialize as `event_id`; on deserialize, accept missing
  or empty fields and generate a fresh `random()` ID.
- Update the `compat` deserializer to populate IDs for legacy entries.
- Migrate every test fixture in the workspace to pass an explicit `EventId`, or
  to use `at` / `now` where appropriate.

Depends on Phase 1.
Touches many files but mechanically.

### Phase 3 — Storage-layer repair

- Add `ensure_unique_event_ids` in `ConversationStream`.
- Call it from `ConversationStream::from_parts` and `from_legacy_events` after
  deserialization, before the stream is returned.
  **Not** part of `ConversationStream::sanitize()`.
- Test: stream with two entries sharing an explicit fixed ID — load-time repair
  leaves the earlier entry's ID intact and regenerates the later one.
- Test: legacy file with no `event_id` fields — entries get IDs assigned at
  load.

Depends on Phase 2.

### Phase 4 — Snapshot regeneration

- Regenerate any snapshot fixtures affected by the new `event_id` field across
  `jp_conversation`, `jp_llm/tests/fixtures/**`, and `jp_md`.
- Verify no test relies on positional references that IDs would break.

Depends on Phases 1–3.

### Phase 5 — Documentation

- Update `jp conversation edit --events` help text with the duplicate-ID note
  (using `event_id`).
- Update the data model section of `docs/architecture/` if applicable.
- Glossary entry in `docs/architecture/ubiquitous-language.md` for "Event ID."

Independent of the others, can land alongside Phase 4.

Future RFDs that want to reference entries by ID — compaction, sub-agents,
plugin event subscriptions, interactive stream editing — migrate their
reference layout in their own implementation work.
That is out of scope here.

## References

- [RFD 047] — Editor and Path Access for Conversations (motivates manual
  editing of `events.json`)
- [RFD 051] — Sub-Agent Workflows (future consumer)
- [RFD 054] — Split Conversation Config and Events (storage shape this RFD
  modifies)
- [RFD 064] — Non-Destructive Conversation Compaction (potential future
  consumer; currently anchors by turn index)
- [RFD 067] — Resource Deduplication (related but distinct concern)
- [RFD 072] — Command Plugin System (governs plugin-protocol exposure of
  `ConversationEvent` JSON)
- [RFD D18] — Plugin Event Subscriptions (future consumer)
- [RFD D21] — Interactive Conversation Stream Editing (future consumer)

[RFD 047]: 047-editor-and-path-access-for-conversations.md
[RFD 051]: 051-sub-agent-workflows.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[RFD 072]: 072-command-plugin-system.md
[RFD D18]: drafts/D18-plugin-event-subscriptions-and-query-delegation.md
[RFD D21]: drafts/D21-interactive-conversation-stream-editing.md
