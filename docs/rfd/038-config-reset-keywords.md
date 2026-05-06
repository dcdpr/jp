# RFD 038: Config Reset Keywords

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08

## Summary

This RFD extends `--cfg` with two reserved UPPERCASE keyword values — `NONE` and
`WORKSPACE` — that each define a **reset point**: a known state that `--cfg`
returns to when the keyword is encountered. `WORKSPACE` expands to a
fully-populated `PartialAppConfig` and is processed using the existing
left-to-right merge model from [RFD 008]. `NONE` is additionally a pre-pipeline
gate that skips all implicit config loading, providing an escape hatch when
implicit config is broken or when a script wants full control.

To persist `NONE`'s reset semantics, `ConfigDelta` is promoted from a struct to
an enum with `Apply` (existing shape) and `Reset` (new) variants.

This RFD focuses on the two keyword reset points. Conversation-ID values for
`--cfg` (e.g. `--cfg=jp-c17528832001`) and fork-implicit config are out of
scope here; see [Non-Goals](#non-goals).

## Motivation

JP loads config from several sources at startup (see [RFD 079] for the full load
sequence). There is no way to deliberately **reset** that accumulated state to a
known baseline mid-invocation. Users hitting broken config have no escape hatch;
scripts can't start from program defaults without brittle workarounds; users who
want to undo post-creation changes to a conversation must hand-pick each field
to reset.

This RFD introduces two reset-point keywords (`NONE` and `WORKSPACE`) so users
can write whatever reset they need directly:

- **Scripting and automation.** A script that wants predictable config
  regardless of the user's environment needs to bypass implicit loading entirely
  (`--cfg=NONE`).
- **Broken-config recovery.** If a config file has a syntax error, `jp` can't
  even start to accept a fix. `--cfg=NONE` provides an escape hatch that skips
  implicit loading entirely.
- **Re-adopting workspace defaults.** A conversation that has diverged from
  workspace config should be able to re-adopt it without hand-picking every
  changed field (`--cfg=WORKSPACE`).

Targeted revert of individual sources — for example, undoing a specific `-c`
that was applied — is covered by [RFD 070]'s `-C` directive, which uses claim
history rather than value-based resets.

## Design

### UPPERCASE keywords for `--cfg`

The existing `--cfg` flag gains reserved UPPERCASE keyword values. `WORKSPACE`
expands to a fully-populated `PartialAppConfig` — the same type that a TOML file
or inline assignment produces. Because the expanded partial sets every config
field, it overwrites whatever came before it in the pipeline.

Both keywords reset the accumulated config state at their position in the
`--cfg` list. They differ in what they reset to.

#### `NONE`

Resets to program defaults (the compiled-in values on `AppConfig::default()`)
and additionally triggers a pre-pipeline gate: if `NONE` appears anywhere in the
`--cfg` list, implicit config loading (described in [RFD 079]) is skipped
entirely. Only explicit `--cfg` values apply on top of program defaults.

Required fields without defaults (for example `assistant.model.id`) must be
supplied by subsequent explicit `--cfg` values. Otherwise validation fails with
a clear error indicating which fields are missing.

#### `WORKSPACE`

Resets to the workspace's fully-resolved config — the merged result of implicit
loading as described in [RFD 079]. This is the same state that new conversations
use by default when no `--cfg` keywords are present.

Note that `config_load_paths` (the deferred-loading search path for `--cfg
<name>`) is a *setting* within the workspace config, not a separate loading
mechanism. `WORKSPACE` includes whatever value `config_load_paths` has in the
merged config, but merely referencing that setting doesn't load any of the files
it points to — those are only loaded on explicit `--cfg <name>` invocation.

#### `--no-cfg` shorthand

`--no-cfg` is shorthand for `--cfg=NONE`. [RFD 070] extends `--no-cfg` to accept
a value for targeted revert (`--no-cfg <source>`, `--no-cfg key=value`); the
bare form retains its meaning from this RFD.

### Ordering

All `--cfg` values — including both keywords — are processed left-to-right ([RFD
008]). A keyword at a given position resets the accumulated state to its target;
subsequent `--cfg` values layer on top.

`NONE` has a second effect beyond the positional reset: a pre-scan of the
`--cfg` list detects any exact `NONE` match and, if found, skips all implicit
loading (see [RFD 079] for what that entails). Implicit loading is a
pipeline-level concern that must be decided before directive processing, hence
the pre-scan.

The positional reset and the pre-scan gate together mean pre-`NONE` directives
are discarded:

```sh
# Pre-scan detects NONE → skip implicit loading.
# Directive loop: `foo.toml` applied, then NONE resets to defaults.
# Result: program defaults only.
jp q --cfg=foo.toml --cfg=NONE

# Pre-scan detects NONE. Directive loop: NONE sets state to defaults,
# then foo.toml applied. Result: defaults + foo.toml.
jp q --cfg=NONE --cfg=foo.toml
```

Since pre-`NONE` directives get discarded anyway, the implementation skips
parsing them entirely. Malformed or missing pre-`NONE` `--cfg` values do not
raise errors — the user's intent was for them to be replaced by `NONE`'s reset.

#### Explicit paths under `NONE`

Since `NONE` skips implicit loading, no config files have been read and the
resolved `config_load_paths` is empty. Subsequent `--cfg <name>` directives that
rely on load-path resolution will fail — there are no search paths to look in.

This is intentional. For scripting under `NONE`, always reference config files
by explicit path:

```sh
# Works: explicit path
jp q --cfg=NONE --cfg=./config/mre.toml

# Fails: `mre` requires config_load_paths to resolve, which NONE skipped
jp q --cfg=NONE --cfg=mre
```

More examples:

```sh
# Start from defaults (implicit loading skipped), layer custom config
jp q --new --cfg=NONE --cfg=./foo.toml

# Pre-NONE directives are discarded; only `fresh.toml` applies
jp q --cfg=./dev.toml --cfg=NONE --cfg=./fresh.toml

# Start from workspace config, then apply overrides
jp q --new --cfg=WORKSPACE --cfg=overrides.toml

# Escape hatch: broken workspace config, use a minimal repro
jp q --cfg=NONE --cfg=./mre.toml "test query"
```

Conversation-ID values (`--cfg=jp-c<id>`) are out of scope for this RFD (see
[Non-Goals](#non-goals)); if a later RFD defines them, they compose with
these keywords in the same pipeline.

### Default behavior

When no keyword is present, `--cfg` values layer on top of the implicit starting
config:

- **New conversations** (`--new`): starts from `WORKSPACE` (current behavior,
  unchanged).
- **Continuing conversations**: starts from the stream's current config state.
- **Forked conversations** (`--fork`): out of scope here (see [Non-Goals](#non-goals)).

The implicit starting config is only relevant when no keyword appears in the
`--cfg` list. A keyword at any position overwrites whatever came before it —
including the implicit starting config — because it sets every field.

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

Conversation IDs (`jp-c` prefix + digits) are out of scope for this RFD (see
[Non-Goals](#non-goals)) but share the same disambiguation pipeline: keyword
matches first, then conversation IDs, then file paths.

### Interaction with continuing conversations

Both `WORKSPACE` and `NONE` work uniformly when continuing an existing
conversation: each persists an event in `events.json` representing its reset
semantics, then any subsequent `--cfg` values layer on top and persist as normal
`Apply` events.

```sh
# Continue conversation, switch to workspace config
jp q --cfg=WORKSPACE

# Continue conversation, reset to program defaults and apply custom config
# (useful for scripts that want predictable state from this point forward)
jp q --cfg=NONE --cfg=mre.toml

```

**For `WORKSPACE`**: the delta between the stream's current resolved config and
the keyword's expanded partial is computed using fully-resolved `AppConfig`
diffing (not partial diffing), which correctly captures fields being set back to
their default values. The resulting delta is stored as a `ConfigDelta::Apply`
event in the stream.

**For `NONE`**: a `ConfigDelta::Reset` event (see [ConfigDelta
enum](#configdelta-enum)) is persisted instead. When folded by future
invocations, `Reset` discards all accumulated state and restarts from
`PartialAppConfig::default()`. Subsequent `--cfg` directives in the same
invocation persist as `Apply` events after the `Reset`, so the full event stream
becomes `[..., Reset, Apply, Apply, ...]`. Future invocations folding the stream
see the reset as authoritative: anything before it is effectively discarded for
config resolution.

Chat history (turns, messages, tool calls) is always loaded — only the config
resolution is affected by `Reset`.

### `ConfigDelta` enum

To persist `NONE`'s reset semantics, `ConfigDelta` is promoted from a struct to
an enum:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ConfigDelta {
    #[serde(rename = "reset")]
    Reset(ResetDelta),

    #[serde(untagged)]
    Apply(ApplyDelta),
}

pub struct ApplyDelta {
    pub timestamp: DateTime<Utc>,
    pub delta: Box<PartialAppConfig>,
}

pub struct ResetDelta {
    pub timestamp: DateTime<Utc>,
}
```

Serde's internal tagging (`tag = "type"`) serializes `Reset` as `{"type":
"reset", "timestamp": "..."}`. `Apply` is marked `#[serde(untagged)]`, so it
serializes as today without a `type` field — preserving the existing on-disk
shape for backward compatibility. No migration needed; legacy events deserialize
into `Apply` via the untagged fallback.

Fold semantics:

- `Apply`: merge `delta` into accumulated state, apply `unsets`, record claims
  (per [RFD 070]).
- `Reset`: discard accumulated state, reset to `PartialAppConfig::default()`.
  Clear any per-invocation claims state. Subsequent `Apply` events apply on top
  of defaults.

For stream walk-back (used by [RFD 070]'s `-C` revert): a `Reset` event
terminates the walk. Anything before the `Reset` is unreachable — `-C` treats it
as equivalent to reaching the base config.

### New conversation creation with `NONE`

When a new conversation is created with `NONE` in the `--cfg` list:

- Pre-scan skips implicit loading, so the workspace `base` partial is
  `PartialAppConfig::default()`.
- Post-`NONE` directives emit `Apply` events that land in `init` (per [RFD
  070]'s `base_config.json` shape).
- No `Reset` event is emitted at creation time — `base` is already defaults, so
  the reset is implicit.

Example:

```sh
jp q -c NONE -c dev --new foobar
```

Produces `base_config.json`:

```json
{
  "base": {},
  "init": [
    { "timestamp": "...", "delta": { /* dev's contribution */ }, "claims": {...} }
  ]
}
```

Future invocations (`jp q baz`) load the conversation, fold defaults + `init` =
defaults + dev. The conversation's working config is "just dev" regardless of
what happens to workspace config files afterward.

## Drawbacks

**`--no-cfg` alone is still incomplete.** Some config fields (e.g. `model`) are
required and have no default values. `--no-cfg` without subsequent `--cfg`
values produces a config that fails validation. This is the intended behavior
for the escape-hatch use case (user will add their own `--cfg`), but the error
message must be clear about what's missing.

**UPPERCASE convention is unusual.** Most CLI tools use lowercase for flag
values. The convention is simple to learn and eliminates disambiguation
entirely, but it may surprise users initially. There is precedent: Vim uses `-u
NONE` and `-U NONE` to distinguish the keyword `NONE` (meaning "no file") from a
lowercase file path, for the same reason we do here.

**Pre-`NONE` directives are silently dropped.** Under the position-sensitive
model, `--cfg dev --cfg NONE` discards the `-c dev`. This is deliberate — the
user's intent is clear from the reset — but it means typos or mistaken argument
order can silently lose configuration. User-facing help should note this: `NONE`
(and by extension `--no-cfg`) discards everything before it.

## Alternatives

### Sigil-prefixed keywords

Use a prefix like `@` (`--cfg=@parent`, `--cfg=@workspace`) to distinguish
keywords from file paths.

Considered but rejected in favor of UPPERCASE, which requires no special
characters and is visually distinct.

## Non-Goals

- **Conversation-ID inheritance.** `--cfg=jp-c<id>` (expanding another
  conversation's resolved config) and `--fork` implicit config are out of
  scope. They share the `--cfg` disambiguation and directive pipeline
  established here, but their semantics — implicit fork config, inner-
  conversation provenance collapse — belong in a future RFD. Where this RFD
  mentions their interaction with the pipeline, it assumes the inheriting
  partials behave like any other fully-populated source.

- **`START` keyword (reset to conversation creation state).** A keyword that
  expands to `base + init` from `base_config.json` — the full state the
  conversation was created with — was considered but deferred. For most users,
  `--cfg WORKSPACE --cfg <original-source>` produces a close-enough result, and
  [RFD 070]'s `-C` handles targeted revert of individual sources. The `init`
  list introduced by [RFD 070] preserves the infrastructure needed to add
  `START` later as a small follow-up RFD if demand emerges.

- **`USER` keyword.** A `USER` keyword that expands to only the user's global
  config (skipping workspace config) was considered but deferred. The use case —
  portable personal defaults across projects — is real but narrow enough to add
  later without design changes.

- **`BASE` keyword.** A `BASE` keyword that expands to just the `base` field of
  `base_config.json` (the creation-time workspace snapshot, without `init`'s
  overrides) was considered but deferred. The use case — strip creation-time
  customization but keep the creation-era workspace config — is genuine but
  narrow. Added later without design changes if needed.

- **Config diffing.** Showing what changed between the inherited config and the
  final resolved config is useful but orthogonal.

## Risks and Open Questions

### Validation timing

With `NONE`, config validation must happen after all `--cfg` values are applied,
not at base resolution time. The current validation flow should already handle
this (validation happens on the final merged config), but it needs verification.

### `NONE` detection ordering

Because `NONE`'s pre-scan gate affects implicit config loading, it must be
detected before `load_base_partial` (which performs the implicit-loading
sequence from [RFD 079]) and before any conversation stream loading. The CLI
entry point scans `--cfg` values for an exact `NONE` match before either step
runs; if found, both are skipped and the base partial starts at
`PartialAppConfig::default()`. The positional reset (emitting a `Reset` event
into the stream, or setting `base = defaults` for new conversations) happens
during the later directive loop.

### Reset and `-C` (from RFD 070)

A `Reset` event in the stream terminates [RFD 070]'s `-C` walk-back: fields
claimed before the `Reset` are unreachable. This matches the semantic model that
`Reset` is an authoritative discard — pre-reset state doesn't contribute to
future config resolution, so it shouldn't contribute to revert either. A `-C
dev` after a `Reset` that discarded dev's claims behaves as "no fields currently
claimed by dev" and emits the standard diagnostic.

## Implementation Plan

### Phase 1: `ConfigDelta` enum and fold-time reset

Promote `ConfigDelta` from a struct to an enum with `Apply` and `Reset` variants
(see [ConfigDelta enum](#configdelta-enum) for the shape).

- Rename the existing `ConfigDelta` struct to `ApplyDelta`.
- Add `ResetDelta { timestamp: DateTime<Utc> }`.
- Define `ConfigDelta` as `#[serde(tag = "type")]` enum with `Reset` tagged as
  `"reset"` and `Apply` marked `#[serde(untagged)]` (backward-compatible with
  existing on-disk events that have no `type` field).
- Update the hand-rolled `deserialize_config_delta` in
  `crates/jp_conversation/src/stream.rs` to dispatch on presence of `type:
  "reset"`. Legacy events without a `type` field deserialize as `Apply`.
- Update the stream fold (in `config()`, `Iter`, `IterMut`, `IntoIter`, and the
  `apply_config_delta` helper introduced by [RFD 070] Phase 1) to match on the
  variant:
  - `Apply`: existing merge + unset + claims logic.
  - `Reset`: replace accumulated state with `PartialAppConfig::default()`; clear
    any in-progress claims state.
- Update [RFD 070]'s walk-back algorithm to terminate at `Reset` events.
- Add a common `timestamp()` accessor on `ConfigDelta` so call sites that only
  need the timestamp don't need to match on the variant.

Tests:

- `ConfigDelta::Reset` round-trip through `deserialize_config_delta`.
- Backward compat: legacy events (no `type` field) load as `Apply`.
- Fold: stream `[Apply(dev), Reset, Apply(fresh)]` resolves to defaults + fresh.
- `-C dev` walk-back after `[Apply(dev), Reset]` reports no matching claims
  (`Reset` terminated the walk).

Can be merged independently.

### Phase 2: Keyword recognition in `--cfg`

Add UPPERCASE keyword recognition to `--cfg` processing. Implement `WORKSPACE`
as a positional `Apply` directive following the ordered-directive pattern from
[RFD 008].

Implement `NONE` in two parts:

- **Pre-pipeline gate.** The CLI entry point scans `--cfg` values for an exact
  `NONE` match; if present, `load_base_partial` and conversation-config loading
  are skipped. Pre-`NONE` `--cfg` values are not parsed (they'd be discarded by
  the positional reset anyway).
- **Positional reset.** In the directive loop, when `NONE` is encountered:
  - For new conversations (`--new`, `--fork`): no explicit event is emitted; the
    new conversation's `base_config.json` is written with `base = defaults`.
    Post-`NONE` directives land in `init` as `Apply` events.
  - For continuing conversations: emit a `Reset` event into `events.json`.
    Post-`NONE` directives emit `Apply` events after it.

Depends on Phase 1.

Conversation-ID resolution is out of scope for this RFD (see [Non-Goals](#non-goals)).

## References

- [RFD 008]: Ordered Tool Directives — establishes left-to-right processing for
  interleaved CLI flags.
- [RFD 079]: Config Sources and Load Order — describes the implicit loading
  sequence that `NONE` skips and `WORKSPACE` re-adopts.
- [RFD 070]: Negative Config Deltas — introduces `init` in `base_config.json`
  (the infrastructure a future `START` keyword would use), extends `ApplyDelta`
  with claims and unsets, and adds a valued form to `--no-cfg` for targeted
  revert.
- `crates/jp_config/src/fs.rs` — config loading and merging logic.
- `crates/jp_config/src/delta.rs` — `ConfigDelta` applied during
  conversations.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 079]: 079-config-sources-and-load-order.md
[RFD 070]: 070-negative-config-deltas.md
