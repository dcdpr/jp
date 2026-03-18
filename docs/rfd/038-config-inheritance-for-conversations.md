# RFD 038: Config Inheritance for Conversations

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08

## Summary

This RFD extends `--cfg` with reserved UPPERCASE keyword values (`NONE`,
`WORKSPACE`) and conversation ID values (`jp-c...`) that expand to
fully-populated `PartialAppConfig`s and are processed using the existing
left-to-right merge model from [RFD 008]. Because these partials set every
field, they effectively override all prior config state — but the mechanism is
identical to any other `--cfg` value.

## Motivation

Today, every new conversation starts from the workspace's default configuration
(`.jp/config.toml` merged with the config load path). There is no way to start a
conversation with a different base — for example, inheriting the resolved config
of an existing conversation (including its accumulated `ConfigDelta`s), or
starting with a completely blank slate for scripting.

This becomes important for:

- **Forked conversations.** A fork should inherit the source conversation's
  config, not the workspace default. If the source conversation had `--cfg`
  overrides applied, the fork should carry those forward.
- **Child conversations.** When conversation trees are introduced, a child
  conversation should inherit its parent's resolved config by default.
- **Scripting and testing.** Scripts that invoke `jp query` may want full
  control over config, starting from nothing rather than inheriting workspace
  defaults that may vary between environments.
- **Reproducibility.** Pointing `--cfg` at a specific conversation makes the
  config source explicit and auditable.

## Design

### UPPERCASE keywords for `--cfg`

The existing `--cfg` flag gains reserved UPPERCASE keyword values. Each keyword
expands to a fully-populated `PartialAppConfig` — the same type that a TOML file
or inline assignment produces — and is merged left-to-right like any other
`--cfg` value. Because the expanded partial sets every config field, it
overwrites whatever came before it in the pipeline.

| Keyword     | Expands to                                                    |
|-------------|---------------------------------------------------------------|
| `NONE`      | A partial where every field is set to its default value.      |
|             | Required fields without defaults (e.g. model) are left unset  |
|             | and must be provided by subsequent `--cfg` values.            |
| `WORKSPACE` | The workspace's fully-resolved config (user global merged     |
|             | with `.jp/config.toml` and the config load path), converted   |
|             | to a partial. This is what new conversations use by default.  |

`--no-cfg` is shorthand for `--cfg=NONE`.

### Ordering

Keywords are processed in the same left-to-right order as any other `--cfg`
value (see [RFD 008]). There is no special "base" or "reset" mechanism — a
keyword simply produces a partial that sets every field, so it overwrites
whatever the pipeline has accumulated up to that point.

```sh
# Start from defaults, then layer custom config
jp q --new --cfg=NONE --cfg=foo.toml

# Start from workspace config, then apply overrides
jp q --new --cfg=WORKSPACE --cfg=overrides.toml

# Inherit another conversation's config
jp q --cfg=jp-c17528832001 --cfg=overrides.toml
```

### Default behavior

When no keyword is present, `--cfg` values layer on top of the implicit starting
config:

- **New conversations** (`--new`): starts from `WORKSPACE` (current behavior,
  unchanged).
- **Forked conversations** (`--fork`): starts from the source conversation's
  resolved config (equivalent to `--cfg=<source-conversation-id>`).
- **Continuing conversations**: starts from the stream's current config state.

The implicit starting config is only relevant when no keyword or conversation ID
appears in the `--cfg` list. A keyword at any position overwrites whatever came
before it — including the implicit starting config — because it sets every
field.

The common case (`jp q` and `jp q --cfg=foo.toml`) works exactly as today.
Keywords are only needed when you want to change the starting point or reset the
config state mid-conversation.

### Disambiguation

UPPERCASE keywords are checked by exact string match before any other
resolution. This eliminates ambiguity without heuristics:

- `NONE` → keyword
- `WORKSPACE` → keyword
- `none` → file path
- `WORKSPACE.toml` → file path (not an exact keyword match)
- `jp-c17528832001` → conversation ID

### Interaction with `--fork`

When `--fork` is used without a keyword, the implicit starting config is the
source conversation's resolved config. This is equivalent to
`--cfg=<source-conversation-id>` — the source conversation's full config (base +
all `ConfigDelta`s) is expanded to a fully-populated partial.

```sh
jp q --fork                               # starts from source conversation
jp q --fork --cfg=WORKSPACE               # starts from workspace config
jp q --fork --cfg=NONE --cfg=custom.toml  # starts from defaults, then custom
```

### Interaction with continuing conversations

Keywords and conversation IDs work the same way when continuing an existing
conversation. The expanded partial overwrites the stream's current config state
(via a `ConfigDelta`), then any subsequent `--cfg` values layer on top.

```sh
# Continue conversation, but switch to workspace config
jp q --cfg=WORKSPACE

# Continue conversation, but adopt another conversation's config
jp q --cfg=jp-c17528832001

# Continue conversation, reset to defaults, then apply custom config
jp q --cfg=NONE --cfg=foo.toml
```

The delta between the stream's current resolved config and the keyword's
expanded partial is computed using fully-resolved `AppConfig` diffing (not
partial diffing), which correctly captures fields being set back to their
default values. The resulting delta is stored as a normal `ConfigDelta` event in
the stream.

### Conversation ID values

`--cfg` accepts a conversation ID to load that conversation's resolved config.
The conversation's full config (base + all `ConfigDelta`s) is converted to a
fully-populated partial — just like a keyword — and merged left-to-right in the
same pipeline.

```sh
# Start from another conversation's config, then layer overrides
jp q --cfg=jp-c17528832001 --cfg=overrides.toml

# Fork but use a different conversation's config instead of the source's
jp q --fork --cfg=jp-c17528832001
```

Conversation IDs have a distinctive format (`jp-c` prefix + digits), so they are
unambiguous with both keywords and file paths. Errors if the conversation is not
found.

## Drawbacks

**`--no-cfg` alone is broken.** Some config fields (e.g. model) are required and
have no default values. `--no-cfg` without subsequent `--cfg` values produces a
config that fails validation. This is documented and the error message guides
the user to add `--cfg`, but it is a footgun.

**UPPERCASE convention is unusual.** Most CLI tools use lowercase for flag
values. The convention is simple to learn and eliminates disambiguation
entirely, but it may surprise users initially. There is precedent: Vim uses
`-u NONE` and `-U NONE` to distinguish the keyword `NONE` (meaning "no file")
from a lowercase file path, for the same reason we do here.

## Alternatives

### Separate `--inherit` flag

A dedicated flag (`--inherit=conversation`, `--inherit=workspace`, etc.) that
controls the base config independently from `--cfg`.

Rejected because it creates two flags that interact in non-obvious ways.
Unifying under `--cfg` with keywords is simpler: one flag, one processing model,
left-to-right.

### Lowercase keywords with heuristic disambiguation

Use lowercase keywords (`none`, `parent`, `workspace`) and disambiguate from
file paths using a resolution order: keywords first, then conversation IDs, then
file paths.

Rejected because it creates ambiguity when a file happens to be named `parent`
or `workspace`. UPPERCASE keywords eliminate this class of problem entirely.

### Sigil-prefixed keywords

Use a prefix like `@` (`--cfg=@parent`, `--cfg=@workspace`) to distinguish
keywords from file paths.

Considered but rejected in favor of UPPERCASE, which requires no special
characters and is visually distinct.

## Non-Goals

- **`PARENT` keyword.** A `PARENT` keyword that expands to the parent
  conversation's resolved config was considered but deferred. For `--fork`, the
  source conversation's config is already the implicit default. For explicit
  use, `--cfg=<conversation-id>` achieves the same result. A `PARENT` keyword
  becomes more useful if conversation trees ([RFD 039]) introduce scenarios
  where the parent ID is not readily known.

- **`USER` keyword.** A `USER` keyword that expands to only the user's global
  config (skipping workspace config) was considered but deferred. The use case —
  portable personal defaults across projects — is real but narrow enough to add
  later without design changes.

- **Config diffing.** Showing what changed between the inherited config and the
  final resolved config is useful but orthogonal.

## Risks and Open Questions

### Validation timing

With `NONE`, config validation must happen after all `--cfg` values are applied,
not at base resolution time. The current validation flow should already handle
this (validation happens on the final merged config), but it needs verification.

### Invalid conversation IDs

When `--cfg=<conversation-id>` references a conversation that does not exist,
the error must be clear and actionable. The message should include the ID that
was not found and suggest checking `jp conversation ls`.

## Implementation Plan

### Phase 1: Keywords and conversation IDs in `--cfg`

Add UPPERCASE keyword recognition and conversation ID resolution to `--cfg`
processing. Implement `NONE`, `WORKSPACE`, and `--cfg=<conversation-id>`.

Follow the ordered-directive pattern from [RFD 008].

Can be merged independently.

### Phase 2: `--fork` config inheritance

Wire `--fork` to use the source conversation's resolved config as the implicit
starting config.

Depends on Phase 1. Can be merged alongside or after [RFD 020]'s `--fork`
implementation.

## References

- [RFD 008]: Ordered Tool Directives — establishes left-to-right processing for
  interleaved CLI flags.
- [RFD 020]: Parallel Conversations — defines `--fork` flag that interacts with
  config keywords.
- [RFD 039]: Conversation Trees — may motivate a `PARENT` keyword in the future.
- `crates/jp_config/src/fs.rs` — config loading and merging logic.
- `crates/jp_config/src/delta.rs` — `ConfigDelta` applied during
  conversations.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 020]: 020-parallel-conversations.md
[RFD 039]: 039-conversation-trees.md
