# RFD D33: Conversation Store and Bare Forking

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17

## Summary

This RFD adds `conversation.store`, an untyped `IndexMap<String, JsonValue>` on
`ConversationConfig`, and `jp conversation fork --bare`, a fork variant that
copies config without conversation events. Together these enable tools to
persist arbitrary data that travels with a conversation and orchestrated
workflows to chain conversations where each phase inherits accumulated state
from its predecessor.

Both features build on the config mutation mechanism from [RFD 078]. The store
is a config field — tools write to it via `access.config` grants and
`outcome.config`. Bare forking copies the resolved config (including the store)
into a fresh conversation.

## Motivation

[RFD 078] gives tools the ability to read and write config paths. But
`AppConfig` has no general-purpose data field — every path is a typed config
setting with specific semantics. Tools that need to persist arbitrary
workflow data (a list of locked decisions, a research summary, a section
tracker) have nowhere to put it without overloading an existing config field.

`conversation.store` fills this gap. It is an untyped map on
`ConversationConfig` — a designated place for tool-produced data that has no
meaning to JP's core systems but travels with the conversation through the
full config lifecycle (persistence, rollback, forking, CLI seeding).

Bare forking addresses a related need: orchestrated workflows that span
multiple conversations. [RFD D05]'s RFD authoring pipeline runs explore,
converge, and draft as separate conversations. Each phase needs the
accumulated data from prior phases but a fresh message history. Regular
forking copies both config and events; bare forking copies only config.

## Design

### `conversation.store`

A new field on `ConversationConfig`:

```rust
pub struct ConversationConfig {
    // ... existing fields ...
    pub store: IndexMap<String, serde_json::Value>,
}
```

The store is an untyped map. Values are opaque JSON. Tools use it to persist
arbitrary data that travels with the conversation.

Because it lives on `AppConfig`, it gets the full config lifecycle:

- **File defaults**: `.jp/config.toml` or persona files can pre-populate
  store keys.
- **CLI initialization**: `jp q --cfg 'conversation.store.rfd_id:="D32"'`
  seeds the store for a run.
- **ConfigDelta persistence**: tool writes via [RFD 078]'s `outcome.config`
  are converted to `ConfigDelta`s. Rollback reverses them.
- **Forking**: regular fork and bare fork both copy the store.

Tools access the store through `access.config` grants scoped to
`conversation.store.*` or more specific paths like
`conversation.store.rfd.decisions`.

### Bare Forking

`jp conversation fork --bare` copies the conversation's resolved config
(including `conversation.store`) without copying any conversation events.

Semantics:
- The parent conversation's config layers are resolved into a single config.
- That resolved config becomes the new conversation's `config_init.json`.
- No events are copied — the new conversation has zero turns.
- The new conversation has no parent-child relationship in the conversation
  tree (unlike a regular fork, which preserves lineage).

This enables orchestrated workflows where each phase starts fresh but
inherits accumulated data:

```
explore conversation (store accumulates research)
  → fork --bare →
converge conversation (inherits research, accumulates decisions)
  → fork --bare →
draft conversation (inherits research + decisions, produces RFD)
```

### Example: RFD Authoring Pipeline

The `rfd_decision` tool is configured with store access:

```toml
[conversation.tools.rfd_decision]
enable = true
run = "ask"
access.config.read = ["conversation.store.rfd.*"]
access.config.write = ["conversation.store.rfd.decisions"]
```

During the converge phase, the tool accumulates decisions:

```json
{
  "content": "Decision #4 locked.",
  "config": {
    "conversation": {
      "store": {
        "rfd": {
          "decisions": [
            { "number": 1, "text": "Flat event structs.", "status": "locked" },
            { "number": 4, "text": "Chrome verbosity API.", "status": "locked" }
          ]
        }
      }
    }
  }
}
```

After the converge conversation ends, `fork --bare` creates the draft
conversation. The draft conversation's tools can read
`conversation.store.rfd.decisions` to see the locked decisions from the
prior phase.

## Drawbacks

- **Config as data store feels unusual.** `AppConfig` is traditionally "how
  the app is configured," not "data the app accumulates." Mitigated by
  namespacing under `conversation.store`.

- **ConfigDelta size.** Tools that write large values to the store create
  large deltas. [RFD 066] addresses content-addressable blob storage for
  large values.

- **Bare fork breaks lineage.** Unlike a regular fork, a bare fork has no
  parent-child relationship. The conversation tree cannot trace the
  relationship between a bare-forked conversation and its source. This is
  intentional (each phase is independent) but means `jp conversation show`
  won't display the chain.

## Alternatives

### Dedicated store outside config

A separate persistence layer for tool data with custom events and rollback.

Rejected — duplicates existing config infrastructure. See [RFD 078]'s
Alternatives for the full argument.

### Filesystem-based state directory

Use `docs/rfd/.state/<NNN>/` (as originally proposed in [RFD D05]) for
workflow data, with tools writing files directly.

Still viable as a complement for human-readable artifacts. But for
machine-readable state that tools consume programmatically, the config-based
store is more integrated (rollback, forking, inspectability via
`jp config show`).

### Fork with event filtering instead of bare fork

Instead of `--bare`, add `--from=start --until=0` or similar to copy config
with an empty event range.

Rejected because the existing `--from`/`--until` flags operate on turn
boundaries within the event stream. "Zero events" is a degenerate case that
deserves its own flag with clear semantics rather than being expressed as an
edge case of range filtering.

## Non-Goals

- **Schema enforcement on store values.** Values are opaque JSON. Typed
  schemas are future work.
- **Cross-conversation store access.** Reading another conversation's store
  without forking is deferred. Bare forking is the only cross-conversation
  data path.
- **Store-aware compaction.** [RFD 064]'s compaction does not need special
  handling for store deltas — they are regular `ConfigDelta`s and compact
  using the same rules.

## Risks and Open Questions

- **Store size limits.** Should there be a cap on total store size per
  conversation? Tools that accumulate large artifacts could bloat the config.
  [RFD 066] mitigates for individual large values but not for many small
  keys.

- **Bare fork discoverability.** If a user bare-forks 10 conversations in a
  pipeline, there is no visible chain linking them. Metadata (e.g., a
  `forked_from` field in the store or conversation metadata) could help but
  is not proposed here.

## Implementation Plan

### Phase 1: `conversation.store` field

Add `store: IndexMap<String, serde_json::Value>` to `ConversationConfig`.
Wire through config merge/delta/serialization. Verify CLI seeding, file
defaults, rollback, and regular forking.

Depends on: [RFD 078] Phase 1 (access.config grants, so tools can actually
use the store).

### Phase 2: `fork --bare`

Add `--bare` flag to `jp conversation fork`. Resolve parent config,
write as new conversation's `config_init.json`, create empty event stream.

Can be merged independently of Phase 1.

## References

- [RFD 078: Tool Config Mutation][RFD 078] — the config mutation mechanism
  that tools use to write to the store.
- [RFD D05: Internal Dev Plugin for RFD Workflows][RFD D05] — primary
  consumer of bare forking for multi-phase workflows.
- [RFD 039: Conversation Trees][RFD 039] — fork semantics.
- [RFD 064: Non-Destructive Conversation Compaction][RFD 064] — compaction
  behavior for config deltas.
- [RFD 066: Content-Addressable Blob Store][RFD 066] — mitigates large
  store values.

[RFD 078]: 078-tool-config-mutation.md
[RFD D05]: D05-internal-dev-plugin-for-rfd-workflows.md
[RFD 039]: 039-conversation-trees.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 066]: 066-content-addressable-blob-store.md
