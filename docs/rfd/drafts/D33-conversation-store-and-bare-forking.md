# RFD D33: Conversation Store and Bare Forking

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Requires**: [RFD 078]
- **Summary**: Adds an untyped `conversation.store` map for tool-persisted
  workflow state and `jp conversation fork --no-turns` for config-only forks.

## Summary

This RFD adds `conversation.store`, an untyped `IndexMap<String, JsonValue>` on
`ConversationConfig` that tools read and write through [RFD 078]'s
`access.config` mechanism, and `jp conversation fork --no-turns`, which copies a
conversation's resolved configuration without its event history.
Together, these let multi-phase workflows persist structured state in a
conversation and carry it into fresh child conversations.

## Motivation

[RFD 078] lets tools read and write config paths, but every path on `AppConfig`
is a typed setting with specific semantics.
Tools that need to persist arbitrary workflow data (locked decisions, a research
summary, a section tracker) have nowhere to store it without overloading an
existing setting.
`conversation.store` gives `ConversationConfig` an untyped map for tool data
that JP's core ignores, while inheriting the full config lifecycle: persistence,
rollback, forking, and CLI seeding.

Multi-phase workflows also need to carry that state across conversations without
carrying the turns that produced it.
[RFD D05]'s RFD authoring pipeline runs explore, converge, and draft as separate
conversations; each phase needs the accumulated state from earlier phases, not
their event history.
Regular forking copies both config and events, whereas `--no-turns` copies only
the resolved config.

## Design

### `conversation.store`

`ConversationConfig` gains an untyped `store` map whose values are opaque JSON:

```rust
pub struct ConversationConfig {
    // ... existing fields ...
    pub store: IndexMap<String, serde_json::Value>,
}
```

Because `conversation.store` lives on `AppConfig`, it participates in the
standard config lifecycle:

- **File defaults**: `.jp/config.toml` and persona files can pre-populate store
  keys.
- **CLI initialization**: `jp q --cfg 'conversation.store.rfd_id:="D32"'` seeds
  the store for a run.
- **ConfigDelta persistence**: tool writes via [RFD 078]'s `outcome.config`
  become `ConfigDelta` events, which conversation rollback reverses.
- **Forking**: both regular and bare forks copy the store.

Tools read and write the store through `access.config` grants scoped to
`conversation.store.*` or narrower paths such as
`conversation.store.rfd.decisions`.

### Orthogonal Fork Opt-Outs

Rather than bundling bare forking into a single `--bare` flag, `jp conversation
fork` provides orthogonal opt-out flags that each suppress one inherited part of
the source conversation:

- `--no-turns`: copy the source's effective config without its conversation
  events, producing a fork with zero turns.
- `--no-parent`: do not record parent-child lineage in the conversation tree.
- `--no-title`: do not inherit the source's title.

Events, lineage, and title are independent concerns, and these flags compose on
any fork: a regular fork can drop only parentage with `--no-parent`, while a
turn-less fork keeps parentage unless `--no-parent` is also passed.
A `--bare` shorthand implying `--no-turns --no-parent` can be added later if
multi-phase workflows need it, but is not required.

When `--no-turns` is passed, JP resolves the source conversation's base config
and every `ConfigDelta` in its stream into a single effective config, writes
that resolved config as the new conversation's base config, and copies no
events.
The resulting conversation is structurally identical to one started fresh with
the source's current effective config.

`--no-parent` depends on conversation-tree lineage ([RFD 039]) and is a no-op
until parentage is tracked.
`--no-title` is deferred to [RFD 053], which owns the title lifecycle.

Together, `--no-turns` and `--no-parent` let each phase of a multi-phase
workflow start with a clean event stream while inheriting accumulated store
data:

```
explore conversation (store accumulates research)
  → fork --no-turns --no-parent →
converge conversation (inherits research, accumulates decisions)
  → fork --no-turns --no-parent →
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

During the converge phase, the tool records locked decisions in the store:

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

When the converge conversation ends, `fork --no-turns --no-parent` creates the
draft conversation, where tools read `conversation.store.rfd.decisions` to
access the locked decisions from the prior phase.

## Drawbacks

- **Config as data store feels unusual.** `AppConfig` traditionally holds
  application settings, not state accumulated during a conversation.
  Namespacing tool data under `conversation.store` keeps the two separate.

- **`ConfigDelta` size.** Tools that write large values to the store produce
  large config deltas.
  [RFD 066] addresses this with content-addressable blob storage for large
  values.

- **Opting out of lineage hides the chain.** Passing `--no-parent` so that
  pipeline phases remain independent prevents the conversation tree from linking
  the fork to its source, so `jp conversation show` does not display the chain.
  This is opt-in and not implied by `--no-turns`.

## Alternatives

### Dedicated store outside config

We considered a separate persistence layer for tool data with its own events and
rollback mechanics.
We rejected this because it duplicates the existing config infrastructure; see
[RFD 078]'s Alternatives section for the full argument.

### Filesystem-based state directory

Tools could write workflow data directly to files under `docs/rfd/.state/<NNN>/`
(as originally proposed in [RFD D05]).
Files on disk remain useful as a complement for human-readable artifacts, but
for machine-readable state that tools consume programmatically,
`conversation.store` integrates directly with rollback, forking, and inspection
via `jp config show`.

### Fork with event filtering instead of `--no-turns`

Instead of adding `--no-turns`, `jp conversation fork` could express a
config-only fork as an empty event range such as `--from=start --to=0`.
We rejected this because `--from` and `--to` select turn boundaries within an
existing event stream; copying zero events is a distinct operation that is
clearer as an explicit flag than as a degenerate range filter.

## Non-Goals

- **Schema enforcement on store values.** Values are opaque JSON; typed schemas
  are future work.
- **Cross-conversation store access.** Reading another conversation's store
  without forking is out of scope.
  Bare forking is the only cross-conversation data path.
- **Store-aware compaction.** Store writes are regular `ConfigDelta` events and
  compact under [RFD 064]'s existing rules without special handling.

## Risks and Open Questions

- **Store size limits.** Should JP cap the total store size per conversation?
  Tools that accumulate large artifacts could bloat the config; [RFD 066]
  addresses individual large values, but not stores with many small keys.

- **Bare fork discoverability.** Forking with `--no-turns --no-parent` across a
  multi-step pipeline leaves no visible chain linking the conversations.
  Recording a `forked_from` field in conversation metadata or in the store could
  preserve that link, but is not proposed here.

## Implementation Plan

### Phase 1: `conversation.store` field

Add `store: IndexMap<String, serde_json::Value>` to `ConversationConfig` and
wire it through config merging, delta generation, and serialization.
Verify CLI seeding, file defaults, rollback, and regular forking.

Depends on [RFD 078] Phase 1 (`access.config` grants).

### Phase 2: `fork --no-turns` (implemented)

Add `--no-turns` to `jp conversation fork` to resolve the source's effective
config (base config plus every `ConfigDelta`) into the new conversation's base
config with an empty event stream.
This phase is independent of Phase 1 and is already implemented on `jp
conversation fork`.

`--no-parent` is deferred until conversation-tree lineage is tracked, and
`--no-title` is deferred to [RFD 053].

## References

- [RFD 078: Tool Config Mutation][RFD 078]: Config mutation mechanism that tools
  use to read and write the store.
- [RFD D05: Internal Dev Plugin for RFD Workflows][RFD D05]: Primary consumer of
  bare forking for multi-phase workflows.
- [RFD 039: Conversation Trees][RFD 039]: Conversation fork and lineage
  semantics.
- [RFD 064: Non-Destructive Conversation Compaction][RFD 064]: Compaction
  behavior for `ConfigDelta` events.
- [RFD 066: Content-Addressable Blob Store][RFD 066]: Content-addressable
  storage for large store values.

[RFD 039]: ../039-conversation-trees.md
[RFD 053]: ../053-auto-refresh-conversation-titles.md
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 078]: ../078-tool-config-mutation.md
[RFD D05]: D05-internal-dev-plugin-for-rfd-workflows.md
