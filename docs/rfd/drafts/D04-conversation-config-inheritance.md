# RFD D04: Conversation Config Inheritance for `--cfg`

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-20

## Summary

This RFD extends `--cfg` to accept conversation IDs (`jp-c...`) as values. When
a conversation ID is passed, the named conversation's fully-resolved config is
loaded and layered into the directive pipeline like any other `--cfg` value.
`--fork` uses the source conversation's ID as the implicit starting config.
Together these enable forked conversations, explicit cross-conversation config
sharing, and reproducible config sources tied to an auditable conversation.

## Motivation

Today, every new conversation starts from the workspace's default configuration.
There is no way to:

- **Fork a conversation** and carry its resolved config (including
  accumulated `ConfigDelta`s) into the new conversation.
- **Inherit another conversation's config** explicitly when starting or
  continuing a conversation — useful for team-shared "template"
  conversations, debugging, or running one conversation's config against
  another's chat history.
- **Reference a specific conversation as a config source** for
  reproducibility. Pointing `--cfg` at a conversation ID makes the source
  explicit and auditable ("this query ran with the same config as
  conversation X").

[RFD 020] introduces `--fork` but leaves the config-inheritance question
open. This RFD answers it: `--fork` defaults to the source conversation's
config, which is equivalent to `--cfg=<source-id>`.

## Design

### Conversation IDs as `--cfg` values

`--cfg` accepts a conversation ID (`jp-c` prefix + digits) as an alternative
to file paths, key-value assignments, JSON objects, and keywords ([RFD 038]).
When passed, the named conversation's full resolved config — base + init +
all event-stream `ConfigDelta`s — is converted to a fully-populated partial
and merged left-to-right in the same pipeline:

```sh
# Start from another conversation's config, then layer overrides
jp q --cfg=jp-c17528832001 --cfg=overrides.toml

# Fork but adopt a different conversation's config instead of the source's
jp q --fork --cfg=jp-c17528832001

# Continue a conversation, but switch config to another conversation's
jp q --cfg=jp-c17528832001
```

Conversation IDs have a distinctive format (`jp-c` prefix + digits), so they
are unambiguous with both keywords (UPPERCASE) and file paths. A file named
literally `jp-c17528832001` would collide, but this is unlikely in practice
— and [RFD 038]'s disambiguation rules resolve exact matches before
falling back to file path resolution.

The resolved-config expansion includes everything the source conversation
accumulated: its base workspace snapshot, its `init` creation-time
directives, and every post-creation `ConfigDelta` in its event stream. The
result is a fully-populated partial that overwrites most fields at its
position in the `--cfg` pipeline, just like `--cfg=WORKSPACE` does.

### `--fork` default

When `--fork` is used without an explicit `--cfg` keyword or conversation
ID, the implicit starting config is the source conversation's resolved
config. This is equivalent to `--cfg=<source-conversation-id>`:

```sh
jp q --fork                               # starts from source conversation
jp q --fork --cfg=WORKSPACE               # starts from workspace config
jp q --fork --cfg=NONE --cfg=custom.toml  # starts from defaults, then custom
jp q --fork --cfg=jp-c17528832001         # starts from a different conversation
```

A keyword at any position overrides the implicit default.

### Disambiguation

[RFD 038] owns the full disambiguation table. The conversation-ID row:

- `jp-c17528832001` → conversation ID (matches `^jp-c\d+$`).
- `jp-c-not-an-id.toml` → file path.
- `./jp-c17528832001` → file path (leading `./` disambiguates).

Exact `jp-c`-prefix matches take precedence over file path resolution.

### Errors

If `--cfg=<conversation-id>` references a conversation that does not exist
in workspace storage, the command errors with:

- The conversation ID that wasn't found.
- A suggestion to check `jp conversation ls`.

No fallback to file path resolution — a missing conversation ID is always
an error, to avoid silently running against a different config than the
user asked for.

### Interaction with RFD 038 keywords

Keywords and conversation IDs compose cleanly in the `--cfg` pipeline —
each position either sets or resets accumulated state:

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

The ordering semantics are inherited from [RFD 008] and [RFD 038] — this
RFD adds no new processing rules beyond resolving the conversation ID to a
partial.

### Interaction with RFD 070 claims

When `--cfg=jp-c<id>` expands a source conversation's resolved config, all
the fields it sets are claimed under a single source identity:
`hash(conversation_id)`. The inner provenance from the source conversation
— which specific `-c dev` or `-c architect` originally claimed each field
— is **not** preserved in the target conversation's claims state.

This is acceptable: inheritance is a wholesale "adopt this state" operation.
`-C jp-c<id>` in the target conversation undoes it wholesale. Users who
need finer-grained control over inherited influence should layer sources
explicitly:

```sh
# Instead of relying on conversation-A's inner provenance:
jp q --cfg=jp-c17528832001 --cfg=overrides.toml

# Layer the same sources explicitly for full provenance:
jp q --cfg=dev --cfg=architect --cfg=overrides.toml
```

See [RFD 070] for the claims-history mechanism.

## Drawbacks

**Hidden dependency on another conversation.** A conversation created with
`--cfg=jp-c<id>` has an implicit dependency on the source conversation
existing in storage. If the source is deleted or on a different machine,
the target conversation works correctly (the expanded partial is baked into
its own `base_config.json`'s `init` at creation), but reproducing the
source relationship requires the source to be present. This mirrors how
file-based `--cfg=dev.toml` depends on the file existing at invocation
time, but conversation storage is less visible to users than a file tree.

**Large inherited partials.** A conversation's resolved config can be
thousands of lines of merged workspace + persona + override state. When
expanded via `--cfg=jp-c<id>`, the full partial is loaded and merged. For
most practical cases this is fine, but there's no size limit — a heavily
customized source conversation produces a correspondingly large partial at
the merge step.

**Circular references are disallowed but need a check.** `--cfg=jp-c<own-id>`
(a conversation referencing itself) would create an infinite-recursion loop
at load time. The resolver must detect and reject self-references with a
clear error.

## Alternatives

### Separate `--inherit` flag

A dedicated flag (`--inherit=conversation`, `--inherit=workspace`, etc.)
that controls the base config independently from `--cfg`.

Rejected because it creates two flags that interact in non-obvious ways.
Unifying under `--cfg` with conversation-ID values (and [RFD 038]'s
keywords) is simpler: one flag, one processing model, left-to-right.

### Reserved `PARENT` keyword

A `PARENT` keyword that expands to the parent conversation's resolved
config.

Deferred. For `--fork`, the source conversation's config is already the
implicit default. For explicit use, `--cfg=<conversation-id>` achieves the
same result. A `PARENT` keyword becomes more useful if conversation trees
([RFD 039]) introduce scenarios where the parent ID is not readily known;
it can be added then without design changes.

### Store the full inherited partial in the target's `init`

When `--cfg=jp-c<id>` creates a new conversation, the source's resolved
config could be materialized verbatim into the new conversation's `init`
list, making the new conversation fully self-contained.

Rejected because it massively inflates `base_config.json`: the target would
duplicate the source's full state on disk. The current design stores only
a `ConfigDelta::Apply` representing the inheritance (the resolved partial
at inheritance time), which is already the fully-populated partial but
persisted as a single event rather than a snapshot. Future invocations fold
it normally.

## Non-Goals

- **Keyword reset points.** `NONE` and `WORKSPACE` are [RFD 038]'s
  concern.
- **Provenance preservation across inheritance.** As noted, inner
  provenance from the source conversation is not retained. Users who want
  fine-grained provenance must layer sources explicitly.
- **Cross-workspace conversation inheritance.** `--cfg=jp-c<id>` resolves
  within the current workspace's storage. Inheriting from a conversation
  in another workspace is out of scope; it would require workspace
  traversal logic that doesn't exist today.

## Risks and Open Questions

### Invalid conversation IDs

When `--cfg=<conversation-id>` references a non-existent conversation, the
error must include:

- The conversation ID that wasn't found.
- A suggestion to check `jp conversation ls`.
- Any close matches (e.g. Levenshtein-distance-1 from an existing ID) if
  typo assistance is cheap to implement.

Typo assistance is a nice-to-have, not a blocker.

### Conversation ID resolution cost

Loading a source conversation requires reading its `base_config.json` and
folding all its `ConfigDelta` events. For long conversations this may
involve parsing hundreds of events. In practice this is fast (well under
100ms for typical streams), but worth validating for conversations with
thousands of events.

### Forward-compatibility with conversation trees

[RFD 039] introduces conversation trees. A tree's root is a conversation,
and its children inherit from the parent. Under this RFD, that inheritance
is `--cfg=<parent-id>` applied implicitly. If trees introduce a notion of
"parent" that's distinct from "source of fork," a `PARENT` keyword may
become useful (see Alternatives).

## Implementation Plan

### Phase 1: `--cfg=<conversation-id>` resolution

Add conversation-ID recognition to `--cfg` processing:

- Disambiguate conversation-ID values by pattern (`jp-c` prefix + digits).
- Resolve the ID against workspace storage to load the conversation
  stream.
- Fold the stream's `base + init + event-stream ConfigDeltas` into a
  fully-resolved partial.
- Insert that partial into the `--cfg` directive pipeline as an `Apply`
  directive.

Error handling:

- Missing conversation: clear error including the ID and `jp conversation
  ls` suggestion.
- Self-reference (`--cfg=<own-id>`): detected via storage-level cycle
  check, rejected with an error.

Can be merged independently.

### Phase 2: `--fork` config inheritance

Wire `--fork` to use the source conversation's resolved config as the
implicit starting config:

- When `--fork` is present without an explicit `--cfg` keyword or
  conversation ID, prepend a synthetic `--cfg=<source-id>` to the
  directive list.
- Subsequent explicit `--cfg` values layer on top.

Depends on Phase 1. Can be merged alongside or after [RFD 020]'s `--fork`
implementation.

## References

- [RFD 008]: Ordered Tool Directives — establishes left-to-right processing
  for interleaved CLI flags.
- [RFD 020]: Parallel Conversations — defines `--fork` flag that interacts
  with conversation inheritance.
- [RFD 038]: Config Reset Keywords — defines `NONE` and `WORKSPACE`
  keywords and the `ConfigDelta` enum with `Reset` variant.
  Conversation IDs compose with keywords in the same `--cfg` pipeline;
  [RFD 038]'s
  disambiguation table covers both.
- [RFD 039]: Conversation Trees — may motivate a `PARENT` keyword for
  tree-aware inheritance.
- [RFD 070]: Negative Config Deltas — defines claim-history-driven revert.
  Conversation-ID inheritance collapses inner provenance under a single
  source identity.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 020]: 020-parallel-conversations.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 039]: 039-conversation-trees.md
[RFD 070]: 070-negative-config-deltas.md
