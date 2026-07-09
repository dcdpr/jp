# RFD 038: Config Reset Keywords

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08

## Summary

This RFD extends `--cfg` with two reserved UPPERCASE keyword values — `NONE`
and `WORKSPACE` — that each define a **reset point**: a known state that
`--cfg` returns to when the keyword is encountered.
Both keywords share the same reset-then-layer mechanism: each persists a
`ConfigDelta::Reset` event that clears accumulated state to
`PartialAppConfig::default()`, and `WORKSPACE` appends a `ConfigDelta::Apply`
carrying the workspace's resolved partial on top.
Subsequent `--cfg` directives layer on top of the reset, processed using the
existing left-to-right merge model from [RFD 008].
`NONE` is additionally a pre-pipeline gate that skips all implicit config
loading, providing an escape hatch when implicit config is broken or when a
script wants full control.

This RFD also defines `loader.reset = "none"` for self-contained config entries.
When a `--cfg` entry declares this setting, JP performs the same positional
reset as `--cfg=NONE` immediately before applying that entry.
Unlike the `NONE` keyword, `loader.reset` does not skip implicit config loading;
it only affects the accumulated state at the entry's position in the `--cfg`
directive stream.

To persist these reset semantics, `ConfigDelta` is promoted from a struct to an
enum with `Apply` (existing shape) and `Reset` (new) variants.

This RFD focuses on reset points in the `--cfg` directive stream: the two
keyword reset points and the entry-local `loader.reset = "none"` setting.
Conversation-ID values for `--cfg` (e.g. `--cfg=jp-c17528832001`) and
fork-implicit config are out of scope here; see [Non-Goals](#non-goals).

## Motivation

JP loads config from several sources at startup (see [RFD 079] for the full load
sequence).
There is no way to deliberately **reset** that accumulated state to a known
baseline mid-invocation.
Users hitting broken config have no escape hatch; scripts can't start from
program defaults without brittle workarounds; users who want to undo
post-creation changes to a conversation must hand-pick each field to reset.

This RFD introduces two reset-point keywords (`NONE` and `WORKSPACE`) and an
entry-local reset setting so users can write whatever reset they need directly:

- **Scripting and automation.** A script that wants predictable config
  regardless of the user's environment needs to bypass implicit loading entirely
  (`--cfg=NONE`).
- **Broken-config recovery.** If a config file has a syntax error, `jp` can't
  even start to accept a fix.
  `--cfg=NONE` provides an escape hatch that skips implicit loading entirely.
- **Re-adopting workspace defaults.** A conversation that has diverged from
  workspace config should be able to re-adopt it without hand-picking every
  changed field (`--cfg=WORKSPACE`).
- **Self-contained config entries.** A named config entry can declare
  `loader.reset = "none"` so `jp q -c entry` applies that entry on top of
  program defaults without requiring the user to remember `--cfg=NONE -c entry`.

Targeted revert of individual sources — for example, undoing a specific `-c`
that was applied — is covered by [RFD 070]'s `-C` directive, which uses claim
history rather than value-based resets.

## Design

### UPPERCASE keywords for `--cfg`

The existing `--cfg` flag gains reserved UPPERCASE keyword values.
Both keywords reset the accumulated config state at their position in the
`--cfg` list; they differ in what they reset to.
Persistence is uniform: each keyword emits a `ConfigDelta::Reset` event into the
conversation stream, and `WORKSPACE` follows that with a `ConfigDelta::Apply`
carrying the workspace's resolved partial.
The `Reset` clears accumulated state to defaults; whatever `Apply` events follow
(whether the keyword's own or user-supplied directives) layer on top in
left-to-right order.

#### `NONE`

Resets to program defaults (the compiled-in values on `AppConfig::default()`)
and additionally triggers a pre-pipeline gate: if `NONE` appears anywhere in the
`--cfg` list, implicit config loading (described in [RFD 079]) is skipped
entirely.
Only explicit `--cfg` values apply on top of program defaults.

Required fields without defaults (for example `assistant.model.id` and
`conversation.tools.defaults.run`) must be supplied by subsequent explicit
`--cfg` values.
Otherwise validation fails with a clear error indicating which fields are
missing.

#### `WORKSPACE`

Resets to the workspace's fully-resolved config — the merged result of implicit
loading as described in [RFD 079].
This is the same state that new conversations use by default when no `--cfg`
keywords are present.

Implicit loading includes `JP_CFG_*` environment variables (see [RFD 079]), so
`WORKSPACE` captures whatever env vars are in effect at invocation time.
The persisted reset records their values; re-running later with different env
vars does not retroactively change an already-stored reset.

Note that `config_load_paths` (the deferred-loading search path for `--cfg
<name>`) is a *setting* within the workspace config, not a separate loading
mechanism.
`WORKSPACE` includes whatever value `config_load_paths` has in the merged
config, but merely referencing that setting doesn't load any of the files it
points to — those are only loaded on explicit `--cfg <name>` invocation.

#### Entry-local reset with `loader.reset`

A config entry can declare that it resets accumulated config before applying
itself:

```toml
[loader]
reset = "none"
```

`loader.reset = "none"` is the entry-local equivalent of placing `--cfg=NONE`
immediately before that entry in the ordered `--cfg` stream.
It resets the accumulated config to program defaults, then applies the entry's
resolved config, including its own `loader.extends` tree.

```sh
# If `committer` declares `loader.reset = "none"`, these are equivalent
# for config state after the `committer` entry applies:
jp q -c committer
jp q -c NONE -c committer
```

The equivalence is positional, not pre-pipeline.
`loader.reset` does not skip implicit config loading, because JP must load the
base config before it can resolve and read the `committer` entry.
If implicit config is broken, the user still needs `--cfg=NONE` / `--no-cfg`.

The setting is honored only when the file is loaded as an explicit `--cfg`
entry.
If the same file is reached through another file's `loader.extends`, its
`loader.reset` value is ignored.
A transitive reset would let an included fragment discard its parent entry's
accumulated config, which is too surprising.

When one `--cfg` argument resolves to multiple entries across roots, each entry
is processed in root precedence order.
A `loader.reset = "none"` on a later entry resets state at that point,
discarding earlier entries from the same argument.

Only `"none"` is defined by this RFD.
Other reset targets can be added later if there is a concrete use case.

#### `--no-cfg` shorthand

`--no-cfg` is shorthand for `--cfg=NONE`.
[RFD 070] extends `--no-cfg` to accept a value for targeted revert (`--no-cfg
<source>`, `--no-cfg key=value`); the bare form retains its meaning from this
RFD.

### Ordering

All `--cfg` values — including both keywords — are processed left-to-right
([RFD 008]).
A keyword at a given position resets the accumulated state to its target;
subsequent `--cfg` values layer on top.

`NONE` has a second effect beyond the positional reset: a pre-scan of the
`--cfg` list detects any exact `NONE` match and, if found, skips all implicit
loading (see [RFD 079] for what that entails).
Implicit loading is a pipeline-level concern that must be decided before
directive processing, hence the pre-scan.

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

Pre-`NONE` `--cfg` values are still parsed by clap (which validates syntax at
parse time), but the directive loop skips processing them: their file-load and
merge effects are not executed, since `NONE`'s positional reset would discard
them anyway.
A syntactically malformed pre-`NONE` `--cfg` value is therefore still rejected
at clap parse time — `NONE` is not a recovery mechanism for malformed arguments
on the same command line.
It recovers from broken *implicit* config (the files JP auto-loads), which is
the actual motivating use case.

#### Explicit paths under `NONE`

Since `NONE` skips implicit loading, no config files have been read and the
resolved `config_load_paths` is empty.
Subsequent `--cfg <name>` directives that rely on search-path resolution will
fail — there are no search paths to look in.

This is intentional.
For scripting under `NONE`, always reference config files by explicit path:

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
[Non-Goals](#non-goals)); if a later RFD defines them, they compose with these
keywords in the same pipeline.

#### `NONE` and `WORKSPACE` are mutually exclusive

`--cfg=NONE` and `--cfg=WORKSPACE` cannot appear in the same `--cfg` list.
`NONE`'s pre-pipeline gate skips the implicit-loading step that `WORKSPACE`
expands to, so the combination is internally inconsistent: either `NONE` is
silently overridden, or `WORKSPACE` lazily re-runs the very loading the user
just opted out of.
Rather than choose, the directive parser rejects the combination outright:

```
error: --cfg=NONE and --cfg=WORKSPACE are mutually exclusive.
```

The check runs before any directive is processed and is independent of position.
To get fresh state with workspace overrides on top, run `--cfg=WORKSPACE` (which
already resets to workspace state).
To start from program defaults and layer arbitrary config explicitly, run
`--cfg=NONE --cfg=<paths>`.

### Default behavior

When no keyword is present, `--cfg` values layer on top of the implicit starting
config:

- **New conversations** (`--new`): starts from `WORKSPACE` (current behavior,
  unchanged).
- **Continuing conversations**: starts from the stream's current config state.
- **Forked conversations** (`--fork`): out of scope here (see
  [Non-Goals](#non-goals)).

The implicit starting config is only relevant when no keyword appears in the
`--cfg` list.
A keyword at any position overwrites whatever came before it — including the
implicit starting config — because it sets every field.

The common case (`jp q` and `jp q --cfg=foo.toml`) works exactly as today.
Keywords are only needed when you want to change the starting point or reset the
config state mid-conversation.

### Disambiguation

UPPERCASE keywords are checked by exact string match before any other
resolution.
This eliminates ambiguity without heuristics:

- `NONE` → keyword
- `WORKSPACE` → keyword
- `none` → file path
- `WORKSPACE.toml` → file path (not an exact keyword match)
- `jp-c17528832001` → conversation ID

For files literally named `NONE` or `WORKSPACE`, force path interpretation with
the existing `@`-prefix or a path-style prefix:

```sh
jp q --cfg=@WORKSPACE     # treat as a path
jp q --cfg=./WORKSPACE    # treat as a path (any leading `./` or `../` works)
```

Conversation IDs (`jp-c` prefix + digits) are out of scope for this RFD (see
[Non-Goals](#non-goals)) but share the same disambiguation pipeline: keyword
matches first, then conversation IDs, then file paths.

### Interaction with continuing conversations

`WORKSPACE`, `NONE`, and `loader.reset = "none"` work uniformly when continuing
an existing conversation: each persists a `ConfigDelta::Reset` event in
`events.json` representing the reset point, then any further state layers on top
as `Apply` events.

```sh
# Continue conversation, switch to workspace config
jp q --cfg=WORKSPACE

# Continue conversation, reset to program defaults and apply custom config
# (useful for scripts that want predictable state from this point forward)
jp q --cfg=NONE --cfg=mre.toml

```

**For `NONE`**: a `ConfigDelta::Reset` is persisted (see [ConfigDelta
enum](#configdelta-enum)) and nothing further from the keyword itself.
Subsequent user-provided `--cfg` directives in the same invocation persist as
`Apply` events after the `Reset`, so the full event stream becomes `[..., Reset,
Apply, Apply, ...]`.

**For `loader.reset = "none"`**: when the resetting entry is processed, JP emits
`ConfigDelta::Reset` before that entry's `Apply`.
The entry itself is then persisted as a normal `Apply` event.
Unlike the `NONE` keyword, no pre-pipeline gate runs.

**For `WORKSPACE`**: a `ConfigDelta::Reset` is persisted, immediately followed
by a `ConfigDelta::Apply` carrying the workspace partial as it resolved at
invocation time.
Subsequent user-provided `--cfg` directives layer on top as additional `Apply`
events, so the stream becomes `[..., Reset, Apply(workspace), Apply, Apply,
...]`.
The persisted workspace `Apply` is *value-stable* — re-running later, even
after workspace config files have been edited, does not retroactively change the
persisted reset.
To re-adopt updated workspace config, run `--cfg=WORKSPACE` again.

When folded by future invocations, `Reset` discards all accumulated state and
restarts from `PartialAppConfig::default()`.
Subsequent `Apply` events apply on top of defaults.
Future invocations folding the stream see the reset as authoritative: anything
before it is effectively discarded for config resolution.

Both keywords and `loader.reset` therefore use the same reset-then-layer
machinery, and `Reset` events terminate [RFD 070]'s `-C` walk-back regardless of
which source emitted them.

Chat history (turns, messages, tool calls) is always loaded — only the config
resolution is affected by `Reset`.

### `ConfigDelta` enum

To persist `Reset` semantics, `ConfigDelta` is promoted from a struct to an
enum:

```rust
#[derive(Debug, Clone)]
pub enum ConfigDelta {
    Apply(ApplyDelta),
    Reset(ResetDelta),
}

pub struct ApplyDelta {
    pub timestamp: DateTime<Utc>,
    pub delta: Box<PartialAppConfig>,
}

pub struct ResetDelta {
    pub timestamp: DateTime<Utc>,
}
```

Serialization is hand-rolled to preserve the existing event envelope.
Today, `InternalEvent::ConfigDelta` is wrapped at the `InternalEvent` layer with
`{"type": "config_delta", ...}` (see `crates/jp_conversation/src/stream.rs`).
The discriminator between `Apply` and `Reset` lives *inside* that envelope as an
`op` field — putting it under `type` would collide with the outer envelope's
own `type` tag and break deserialization (the existing
`InternalEvent::deserialize` dispatches on the top-level `type`).

Wire shapes:

```json
// Apply (unchanged from today; no `op` field is written, though an explicit
// "apply" is accepted on read).
{"type": "config_delta", "timestamp": "...", "delta": { /* ... */ }}

// Reset (new).
{"type": "config_delta", "op": "reset", "timestamp": "..."}
```

The `Apply` form is identical to today's on-disk shape.
`deserialize_config_delta` in `crates/jp_conversation/src/stream.rs` matches the
`op` field strictly: absent (which covers all legacy events, so no migration is
needed) or `"apply"` decodes as `Apply`; `"reset"` decodes as `Reset`; any other
value is a deserialization error.
Rejecting unknown ops keeps the field open for future variants: an op written by
a newer version fails loudly on older versions instead of being misread as an
`Apply`.

Fold semantics:

- `Apply`: merge `delta` into accumulated state, apply `unsets`, record claims
  (per [RFD 070]).
- `Reset`: discard accumulated state, reset to `PartialAppConfig::default()`.
  Clear any per-invocation claims state.
  Subsequent `Apply` events apply on top of defaults.

For stream walk-back (used by [RFD 070]'s `-C` revert): a `Reset` event
terminates the walk.
Anything before the `Reset` is unreachable — `-C` treats it as equivalent to
reaching the base config.

### New conversation creation with `NONE` and `loader.reset`

When a new conversation is created with `NONE` in the `--cfg` list:

- Pre-scan skips implicit loading, so the starting partial is
  `PartialAppConfig::default()`.
- Post-`NONE` directives are merged on top of defaults during the directive
  loop, and the result is written to `base_config.json` as today's flat partial
  (per [RFD 054]).
- No `Reset` event is emitted at creation time — the base file already
  represents the post-reset state.
  Subsequent invocations load the conversation and start from this flat base.

Example:

```sh
jp q -c NONE -c dev --new foobar
```

Produces `base_config.json` containing dev's contribution merged on top of
program defaults.
The conversation's working config starts at "just dev" regardless of what
happens to workspace config files afterward.

`loader.reset = "none"` follows the same creation-time rule at its directive
position.
If `committer` declares `loader.reset = "none"`, then:

```sh
jp q -c committer --new foobar
```

writes the post-reset state to `base_config.json` without emitting a `Reset`
event at creation time.

> [!NOTE]
> [RFD 070] reshapes `base_config.json` to `{ base, init }` and adds per-source
> claims.
> Once that lands, post-`NONE` directives at creation time will land in `init`
> as `Apply` events with their own claims rather than being merged into `base`.
> Until then, fields touched by post-`NONE` directives at creation time have no
> per-source provenance.

## Drawbacks

**`--no-cfg` alone is still incomplete.** Some config fields (e.g. `model`) are
required and have no default values.
`--no-cfg` without subsequent `--cfg` values produces a config that fails
validation.
This is the intended behavior for the escape-hatch use case (user will add their
own `--cfg`), but the error message must be clear about what's missing.

**UPPERCASE convention is unusual.** Most CLI tools use lowercase for flag
values.
The convention is simple to learn and eliminates disambiguation entirely, but it
may surprise users initially.
There is precedent: Vim uses `-u NONE` and `-U NONE` to distinguish the keyword
`NONE` (meaning "no file") from a lowercase file path, for the same reason we do
here.

**Pre-`NONE` directives are silently dropped.** Under the position-sensitive
model, `--cfg dev --cfg NONE` discards the `-c dev`.
This is deliberate — the user's intent is clear from the reset — but it means
typos or mistaken argument order can silently lose configuration.
User-facing help should note this: `NONE` (and by extension `--no-cfg`) discards
everything before it.

## Alternatives

### Sigil-prefixed keywords

Use a prefix like `@` (`--cfg=@parent`, `--cfg=@workspace`) to distinguish
keywords from file paths.

Considered but rejected in favor of UPPERCASE, which requires no special
characters and is visually distinct.

### Lowercase reserved keywords

Use lowercase reserved words (`--cfg=none`, `--cfg=workspace`) and rely on the
`@`-prefix or `./`-prefix escape for files literally named `none` or
`workspace`.

Rejected because lowercase tokens are visually indistinguishable from common
file names.
UPPERCASE makes the keyword nature obvious to readers and minimizes the risk of
accidental shadowing — the relevant precedent is Vim's `-u NONE`, which uses
the same uppercase convention to disambiguate against a file path.

### Separate `--config-base=none|workspace` flag

Express the reset point via a dedicated flag instead of a value within `--cfg`.

Rejected because a separate flag would split the ordered directive stream into
two parallel inputs and break the left-to-right composition with other `-c`
directives that this design relies on (per [RFD 008]).
Putting the reset point at a position in the `--cfg` list, alongside everything
else, is what makes commands like `--cfg=foo.toml --cfg=NONE --cfg=bar.toml`
self-explanatory.

## Non-Goals

- **Conversation-ID inheritance.** `--cfg=jp-c<id>` (expanding another
  conversation's resolved config) and `--fork` implicit config are out of scope.
  They share the `--cfg` disambiguation and directive pipeline established here,
  but their semantics — implicit fork config, inner- conversation provenance
  collapse — belong in a future RFD.
  Where this RFD mentions their interaction with the pipeline, it assumes the
  inheriting partials behave like any other fully-populated source.

- **`START` keyword (reset to conversation creation state).** A keyword that
  expands to `base + init` from `base_config.json` — the full state the
  conversation was created with — was considered but deferred.
  For most users, `--cfg WORKSPACE --cfg <original-source>` produces a
  close-enough result, and [RFD 070]'s `-C` handles targeted revert of
  individual sources.
  The `init` list introduced by [RFD 070] preserves the infrastructure needed to
  add `START` later as a small follow-up RFD if demand emerges.

- **`USER` keyword.** A `USER` keyword that expands to only the user's global
  config (skipping workspace config) was considered but deferred.
  The use case — portable personal defaults across projects — is real but
  narrow enough to add later without design changes.

- **`BASE` keyword.** A `BASE` keyword that expands to just the `base` field of
  `base_config.json` (the creation-time workspace snapshot, without `init`'s
  overrides) was considered but deferred.
  The use case — strip creation-time customization but keep the creation-era
  workspace config — is genuine but narrow.
  Added later without design changes if needed.

- **Config diffing.** Showing what changed between the inherited config and the
  final resolved config is useful but orthogonal.

## Risks and Open Questions

### Validation timing

With `NONE`, config validation must happen after all `--cfg` values are applied,
not at base resolution time.
The current validation flow should already handle this (validation happens on
the final merged config), but it needs verification.

Phase 1 adds a related constraint for continuing conversations:
`add_config_delta` suppresses empty `Apply` diffs by resolving the stream's
current config, and that resolution fails between a `Reset` and whichever
`Apply` restores the required fields.
Phase 2 must append post-reset `Apply` events directly (or defer diffing) rather
than routing them through the suppression path.

### `NONE` detection ordering

Because `NONE`'s pre-scan gate affects implicit config loading, it must be
detected before `load_base_partial` (which performs the implicit-loading
sequence from [RFD 079]) and before conversation-config folding.
The CLI entry point scans `--cfg` values for an exact `NONE` match before either
step runs; if found, both are skipped and the base partial starts at
`PartialAppConfig::default()`.
The conversation stream itself is still loaded — chat history (turns, messages,
tool calls) is unaffected.
The positional reset (emitting a `Reset` event into the stream for continuing
conversations, or absorbing post-`NONE` directives into `base_config.json` for
new conversations) happens during the later directive loop.

### Reset and `-C` (from RFD 070)

A `Reset` event in the stream terminates [RFD 070]'s `-C` walk-back: fields
claimed before the `Reset` are unreachable.
This matches the semantic model that `Reset` is an authoritative discard —
pre-reset state doesn't contribute to future config resolution, so it shouldn't
contribute to revert either.
A `-C dev` after a `Reset` that discarded dev's claims behaves as "no fields
currently claimed by dev" and emits the standard diagnostic.

### `loader.reset` metadata timing

`loader.reset` is loader metadata.
It must be read from a resolved `--cfg` entry before that entry is applied, but
it is not part of the persisted application config.
The field itself is not written to conversation deltas or `base_config.json`;
only its reset effect is persisted through `Reset` events or creation-time base
state.

## Implementation Plan

### Phase 1: `ConfigDelta` enum and fold-time reset

Promote `ConfigDelta` from a struct to an enum with `Apply` and `Reset` variants
(see [ConfigDelta enum](#configdelta-enum) for the shape).

- Rename the existing `ConfigDelta` struct to `ApplyDelta`.
- Add `ResetDelta { timestamp: DateTime<Utc> }`.
- Define `ConfigDelta` as a plain enum (no `#[serde(tag)]`).
  Hand-roll `Serialize` so `Apply` produces today's flat shape (no `op` field)
  and `Reset` produces `{"op": "reset", "timestamp": "..."}`.
  The outer `InternalEvent` wrapper continues to add `"type": "config_delta"`
  unchanged.
- Update the hand-rolled `deserialize_config_delta` in
  `crates/jp_conversation/src/stream.rs` to dispatch on the `op` field: absent
  (all legacy events) or `"apply"` decodes as `Apply`, `"reset"` decodes as
  `Reset`, and any other value is a deserialization error.
- Update `add_config_delta` in the same file: the existing destructure and
  empty-diff suppression apply only to `Apply`.
  `add_config_delta(Reset)` always appends — `Reset` events have no diff to
  suppress and their presence is the whole point.
- Centralize the stream fold in a `fold_config_delta` helper shared by
  `config()`, `Iter`, `IterMut`, and `IntoIter`, matching on the variant:
  - `Apply`: existing merge logic.
  - `Reset`: replace accumulated state with `PartialAppConfig::default()`.
- [RFD 070] hooks into the same helper when it lands (it requires this RFD):
  `Apply` gains unset and claims handling, `Reset` clears per-invocation claims
  state, and its walk-back algorithm terminates at `Reset` events.
- Add a common `timestamp()` accessor on `ConfigDelta` so call sites that only
  need the timestamp don't need to match on the variant.

Tests:

- `ConfigDelta::Reset` round-trips through full `InternalEvent`
  serialize/deserialize (not just `deserialize_config_delta` in isolation),
  asserting the on-disk shape is `{"type":"config_delta","op":"reset",...}`.
- `ConfigDelta::Apply` on-disk shape is unchanged from today.
- Backward compat: legacy events without `op` decode as `Apply`, and an explicit
  `op == "apply"` is accepted.
- Unknown `op` values fail deserialization.
- `add_config_delta(ConfigDelta::Reset(_))` always appends, even on a stream
  where adding an empty `Apply` would be suppressed.
- Fold: stream `[Apply(dev), Reset, Apply(fresh)]` resolves to defaults + fresh.
- `-C dev` walk-back after `[Apply(dev), Reset]` reports no matching claims
  (`Reset` terminated the walk; implemented as part of [RFD 070]'s claims work).

Can be merged independently.

### Phase 2: Reset recognition in `--cfg`

Add UPPERCASE keyword recognition and entry-local `loader.reset` recognition to
`--cfg` processing.
All reset points share the same reset-then-layer machinery; they differ in what
triggers them and what, if anything, is layered on top.

**Up-front mutual-exclusion check.** If both `NONE` and `WORKSPACE` appear in
the same `--cfg` list, reject with a clear error before any directive runs (see
[`NONE` and `WORKSPACE` are mutually
exclusive](#none-and-workspace-are-mutually-exclusive)).
The check is purely based on the parsed `--cfg` list and is independent of
position.

Implement `NONE`:

- **Pre-pipeline gate.** The CLI entry point scans `--cfg` values for an exact
  `NONE` match; if present, `load_base_partial` and conversation-config folding
  are skipped.
  The conversation stream is still loaded for chat history.
  Pre-`NONE` `--cfg` values are still parsed by clap (syntactic validation runs
  at parse time); the directive loop simply skips processing them.
- **Positional reset.** In the directive loop, when `NONE` is encountered:
  - For new conversations (`--new`, `--fork`): no event is emitted; the new
    conversation's `base_config.json` is written with the post-`NONE` merge
    result as today's flat partial (per [RFD 054]).
    [RFD 070] later refactors this to `{ base, init }` so post-`NONE` directives
    gain per-source claims.
  - For continuing conversations: emit a `ConfigDelta::Reset` event into
    `events.json`.
    Post-`NONE` directives emit `Apply` events after it.

Implement `WORKSPACE`:

- For new conversations (`--new`, `--fork`): no events are emitted from the
  keyword itself — the default new-conversation behavior already starts from
  workspace state and writes `base_config.json` accordingly.
  `--cfg=WORKSPACE` at creation time is therefore equivalent to omitting it, and
  post-`WORKSPACE` directives merge in as today.
- For continuing conversations: emit a `ConfigDelta::Reset` event followed by a
  `ConfigDelta::Apply` carrying the workspace partial as resolved at invocation
  time.
  Post-`WORKSPACE` directives emit `Apply` events after the pair.

Depends on Phase 1.

Implement `loader.reset = "none"`:

- Read the setting from each resolved `--cfg` entry before applying that entry's
  partial.
- Honor it only for explicit `--cfg` entries.
  Ignore it when the same file is reached through `loader.extends`.
- For continuing conversations, emit `ConfigDelta::Reset` immediately before the
  resetting entry's `Apply` event.
- For new conversations, absorb the reset into the new base state the same way
  `NONE` is absorbed at creation time.
- Do not run the `NONE` pre-pipeline gate.
  Broken implicit config still requires the `NONE` keyword or bare `--no-cfg`.

Tests:

- A continuing conversation with `-c committer` where `committer` declares
  `loader.reset = "none"` appends `[Reset, Apply(committer)]`.
- A new conversation with the same entry writes post-reset state to
  `base_config.json` and emits no `Reset` event at creation time.
- A file reached through `loader.extends` with `loader.reset = "none"` does not
  reset the parent entry.
- A multi-root `--cfg dev` where the later root's entry declares `loader.reset =
  "none"` discards earlier resolved entries from that same argument.
- `loader.reset = "none"` does not skip implicit loading; a broken implicit
  config still fails unless the command uses `NONE` / `--no-cfg`.

Conversation-ID resolution is out of scope for this RFD (see
[Non-Goals](#non-goals)).

## References

- [RFD 008]: Ordered Tool Directives — establishes left-to-right processing for
  interleaved CLI flags.
- [RFD 079]: Config Sources and Load Order — describes the implicit loading
  sequence that `NONE` skips and `WORKSPACE` re-adopts.
- [RFD 070]: Negative Config Deltas — introduces `init` in `base_config.json`
  (the infrastructure a future `START` keyword would use), extends `ApplyDelta`
  with claims and unsets, and adds a valued form to `--no-cfg` for targeted
  revert.
- [RFD 054]: Split Conversation Config and Events — defines today's flat
  `base_config.json` shape that this RFD writes post-`NONE` state into until RFD
  070 reshapes it.
- `crates/jp_config/src/fs.rs` — config loading and merging logic.
- `crates/jp_cli/src/config_pipeline.rs` — resolved `--cfg` entries and loader
  metadata.
- `crates/jp_conversation/src/stream.rs` — conversation `ConfigDelta` and
  stream fold.
- `crates/jp_config/src/delta.rs` — `PartialConfigDelta` trait and
  partial-merge primitives.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 070]: 070-negative-config-deltas.md
[RFD 079]: 079-config-sources-and-load-order.md
