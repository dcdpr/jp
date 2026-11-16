# RFD D04: Conversation Config Inheritance

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-20
- **Requires**: [RFD 070]
- **Extends**: [RFD 038]
- **Summary**: Accepts conversation IDs in `--cfg` and defaults `--fork` to
  inherit the source conversation's resolved configuration.

## Summary

This RFD extends `--cfg` to accept conversation IDs (`jp-c...`).
Passing a conversation ID loads that conversation's resolved configuration and
layers it into the `--cfg` pipeline.
`--fork` defaults to the source conversation's resolved configuration, so forked
conversations inherit accumulated `ConfigDelta`s.

## Motivation

Every new conversation starts from the workspace's default configuration.
There is no way to:

- **Fork a conversation** and carry its resolved config, including accumulated
  `ConfigDelta`s, into the new conversation.
- **Inherit another conversation's config** when starting or continuing a
  conversation, such as for team-shared template conversations, debugging, or
  running one conversation's config against another's chat history.
- **Reference a conversation as a config source** for reproducibility, making it
  explicit and auditable that a query ran with the same config as a given
  conversation.

[RFD 020] introduces `--fork` but leaves config inheritance open.
This RFD defaults `--fork` to inherit the source conversation's config,
equivalent to `--cfg=<source-id>`.

## Design

### Conversation IDs as `--cfg` values

`--cfg` accepts a conversation ID (`jp-c` prefix followed by digits) alongside
file paths, key-value assignments, JSON objects, and keywords ([RFD 038]).
Passing a conversation ID expands that conversation's resolved config (its base
workspace snapshot, creation-time `init` directives, and all event-stream
`ConfigDelta`s) into a fully-populated partial and merges it left-to-right in
the `--cfg` pipeline.
Like `--cfg=WORKSPACE`, this partial overwrites most fields at its position in
the pipeline:

```sh
# Start from another conversation's config, then layer overrides
jp q --cfg=jp-c17528832001 --cfg=overrides.toml

# Fork but adopt a different conversation's config instead of the source's
jp q --fork --cfg=jp-c17528832001

# Continue a conversation, but switch config to another conversation's
jp q --cfg=jp-c17528832001
```

### `--fork` default

Without an explicit `--cfg` keyword or conversation ID, `--fork` starts from the
source conversation's resolved config, equivalent to
`--cfg=<source-conversation-id>`.
Passing a keyword or conversation ID at any position overrides this default:

```sh
jp q --fork                               # starts from source conversation
jp q --fork --cfg=WORKSPACE               # starts from workspace config
jp q --fork --cfg=NONE --cfg=custom.toml  # starts from defaults, then custom
jp q --fork --cfg=jp-c17528832001         # starts from a different conversation
```

### Disambiguation

Conversation IDs (`^jp-c\d+$`) are distinct from uppercase keywords and most
file paths.
[RFD 038] defines the full disambiguation table, where exact `jp-c`-prefix
matches take precedence over file path resolution:

- `jp-c17528832001` → conversation ID (matches `^jp-c\d+$`).
- `jp-c-not-an-id.toml` → file path.
- `./jp-c17528832001` → file path (a leading `./` forces file path resolution,
  avoiding collision with a file literally named `jp-c17528832001`).

### Errors

If `--cfg=<conversation-id>` references a conversation that does not exist in
workspace storage, the command fails with an error naming the missing ID and
suggesting `jp conversation ls`.
It does not fall back to file path resolution, which would silently run with a
different config than requested.

### Interaction with RFD 038 keywords

Keywords and conversation IDs compose in the `--cfg` pipeline.
Each argument either layers onto or resets the accumulated state, following the
left-to-right ordering rules from [RFD 008] and [RFD 038] without adding new
processing rules beyond resolving a conversation ID to a partial:

```sh
# Start from conversation A, reset to defaults (NONE pre-scan), apply custom
jp q --cfg=jp-c17528832001 --cfg=NONE --cfg=custom.toml
# Result: defaults + custom (NONE discarded the conversation load)

# Start from conversation A, overlay conversation B, then local override
jp q --cfg=jp-c17528832001 --cfg=jp-c99999999999 --cfg=overrides.toml
# Result: A's resolved + B's resolved + overrides

# Fork from a conversation, reset to workspace defaults, apply a new skill
jp q --fork=jp-c17528832001 --cfg=WORKSPACE --cfg=debug.toml
# Result: workspace defaults + debug
```

### Interaction with RFD 070 claims

When `--cfg=jp-c<id>` expands a source conversation's resolved config, every
field it sets is claimed under a single source identity: `hash(conversation_id)`
(see [RFD 070]).
The target conversation does not preserve the source conversation's inner
provenance (which specific `-c dev` or `-c architect` flag originally claimed
each field).

Inheritance adopts the source conversation's state as a unit, and `-C jp-c<id>`
in the target conversation reverts it as a unit.
To retain per-source provenance, layer the original sources explicitly:

```sh
# Instead of relying on conversation-A's inner provenance:
jp q --cfg=jp-c17528832001 --cfg=overrides.toml

# Layer the same sources explicitly for full provenance:
jp q --cfg=dev --cfg=architect --cfg=overrides.toml
```

## Drawbacks

**External dependency on source conversations.** Resolving `--cfg=jp-c<id>`
requires the source conversation to exist in workspace storage at invocation
time, just as `--cfg=dev.toml` requires the file to exist.
Once created, the target conversation continues to work if the source is deleted
or on another machine because the expanded partial is recorded in its
`base_config.json`'s `init`, but reproducing the source relationship requires
the source conversation to remain in storage, which is less visible than a file
tree.

**Large inherited partials.** A conversation's resolved config can span
thousands of lines of merged workspace, persona, and override state.
Expanding `--cfg=jp-c<id>` loads and merges that full partial with no size
limit, so a heavily customized source conversation produces a correspondingly
large partial at the merge step.

**Self-reference cycle check.** Passing `--cfg=jp-c<own-id>` (a conversation
referencing itself) would cause infinite recursion at load time, so the resolver
must detect and reject self-references with a clear error.

## Alternatives

### Separate `--inherit` flag

A dedicated flag (`--inherit=conversation`, `--inherit=workspace`, etc.) could
control the base config independently from `--cfg`.
This is rejected because two config flags would interact in non-obvious ways.
Handling conversation IDs and [RFD 038] keywords in `--cfg` keeps a single
left-to-right pipeline.

### Reserved `PARENT` keyword

A `PARENT` keyword could expand to the parent conversation's resolved config.
This is deferred: `--fork` already defaults to the source conversation's config,
and `--cfg=<conversation-id>` covers explicit references.
If conversation trees ([RFD 039]) introduce cases where the parent ID is not
readily known, `PARENT` can be added without changing this design.

### Store the full inherited partial in the target's `init`

When `--cfg=jp-c<id>` creates a new conversation, the source's resolved config
could be materialized verbatim into the new conversation's `init` list in
`base_config.json`, making the new conversation self-contained.
This is rejected because duplicating the source's full state on disk inflates
`base_config.json`.
Instead, the target stores a single `ConfigDelta::Apply` event containing the
resolved partial at inheritance time, which future invocations fold with the
rest of the event stream.

## Non-Goals

- **Keyword reset points.** `NONE` and `WORKSPACE` are defined in [RFD 038].
- **Provenance preservation across inheritance.** Inner provenance from the
  source conversation is not retained; users who want fine-grained provenance
  must layer sources explicitly.
- **Cross-workspace conversation inheritance.** `--cfg=jp-c<id>` resolves only
  within the current workspace's storage.
  Inheriting from a conversation in another workspace requires cross-workspace
  traversal and is out of scope.

## Risks and Open Questions

### Invalid conversation IDs

When `--cfg=<conversation-id>` references a non-existent conversation, the error
reports the missing ID and suggests `jp conversation ls`.
If cheap to implement, the error can also include close matches (such as
existing IDs within Levenshtein distance 1), though typo assistance is not a
blocker.

### Conversation ID resolution cost

Loading a source conversation reads its `base_config.json` and folds every
`ConfigDelta` in its event stream.
Typical streams resolve in well under 100ms, but performance on conversations
with thousands of events should be validated.

### Forward-compatibility with conversation trees

In [RFD 039] conversation trees, child conversations inherit from their parent,
which maps to an implicit `--cfg=<parent-id>`.
If trees distinguish a conversation's tree parent from its fork source, a
`PARENT` keyword can represent that relationship (see Alternatives).

## Implementation Plan

### Phase 1: `--cfg=<conversation-id>` resolution

Add conversation-ID recognition to `--cfg` processing:

- Disambiguate conversation-ID values by pattern (`jp-c` prefix + digits).
- Resolve the ID against workspace storage to load the conversation stream, or
  fail with a clear error naming the ID and suggesting `jp conversation ls`.
- Reject self-references (`--cfg=<own-id>`) via a storage-level cycle check.
- Fold the stream's `base + init + event-stream ConfigDeltas` into a
  fully-resolved partial and insert it into the `--cfg` directive pipeline as an
  `Apply` directive.

This phase can be merged independently.

### Phase 2: `--fork` config inheritance

Wire `--fork` to use the source conversation's resolved config as its default
starting config:

- When `--fork` is present without an explicit `--cfg` keyword or conversation
  ID, prepend a synthetic `--cfg=<source-id>` to the directive list.
- Layer subsequent explicit `--cfg` values on top.

This phase depends on Phase 1 and can be merged alongside or after [RFD 020]'s
`--fork` implementation.

## References

- [RFD 008]: Ordered Tool Directives.
  Establishes left-to-right processing for interleaved CLI flags.
- [RFD 020]: Parallel Conversations.
  Defines the `--fork` flag that interacts with conversation inheritance.
- [RFD 038]: Config Reset Keywords.
  Defines the `NONE` and `WORKSPACE` keywords, the `ConfigDelta` enum with
  `Reset` variant, and the `--cfg` disambiguation table covering both keywords
  and conversation IDs.
- [RFD 039]: Conversation Trees.
  Defines conversation trees that could motivate a `PARENT` keyword for
  tree-aware inheritance.
- [RFD 070]: Negative Config Deltas.
  Defines claim-history-driven revert, under which conversation-ID inheritance
  collapses inner provenance under a single source identity.

[RFD 008]: ../008-ordered-tool-directives.md
[RFD 020]: ../020-parallel-conversations.md
[RFD 038]: ../038-config-reset-keywords.md
[RFD 039]: ../039-conversation-trees.md
[RFD 070]: ../070-negative-config-deltas.md
