# RFD 038: Config Reset Keywords

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08
- **Summary**: Adds `NONE` and `WORKSPACE` reset keywords to `--cfg`, an
  entry-level `loader.reset` setting, and `ConfigDelta::Reset` events.

## Summary

This RFD adds two reserved uppercase reset keywords (`NONE` and `WORKSPACE`) to
`--cfg`, along with an entry-local `loader.reset = "none"` setting.
Each resets accumulated configuration at its position in the left-to-right
`--cfg` pipeline ([RFD 008]).
`NONE` and `loader.reset = "none"` reset to program defaults (with `NONE` also
skipping implicit startup loading), while `WORKSPACE` resets to defaults and
reapplies the workspace's resolved configuration.
To persist these resets in conversation history, `ConfigDelta` becomes an enum
with `Apply` and `Reset` variants.

## Motivation

JP loads configuration from several sources at startup ([RFD 079]) and layers
conversation deltas on top, but provides no way to reset that accumulated state
to a known baseline mid-invocation.
This blocks four workflows:

- **Scripting and automation.** Scripts that need predictable configuration
  regardless of the user's environment have no clean way to bypass implicit
  loading (`--cfg=NONE`).
- **Broken-config recovery.** A syntax error in an implicitly loaded config file
  prevents `jp` from starting.
  `--cfg=NONE` skips implicit loading so `jp` can run with explicit overrides.
- **Re-adopting workspace defaults.** Once a conversation diverges from the
  workspace config, returning to the workspace baseline requires resetting every
  changed field by hand (`--cfg=WORKSPACE`).
- **Self-contained config entries.** A named config entry that expects a clean
  baseline can declare `loader.reset = "none"` so `jp q -c entry` applies on top
  of program defaults without requiring `--cfg=NONE -c entry`.

Reverting an individual config source (such as undoing a single `-c` argument)
is handled by [RFD 070]'s `-C` directive, which tracks per-field claim history
rather than resetting accumulated values.

## Design

### Reset keywords and `loader.reset`

`--cfg` gains two reserved uppercase keywords, `NONE` and `WORKSPACE`, and
config entries gain a `loader.reset = "none"` setting.
Each resets the accumulated config state at its position in the `--cfg` list.

Persistence is uniform across all three mechanisms: each emits a
`ConfigDelta::Reset` event into the conversation stream to clear accumulated
state to program defaults, and `WORKSPACE` follows that with a
`ConfigDelta::Apply` carrying the workspace's resolved partial.
Any subsequent directives layer on top as `Apply` events in left-to-right order.

#### `NONE`

`NONE` resets accumulated config to program defaults (`AppConfig::default()`)
and triggers a pre-pipeline gate: if `NONE` appears anywhere in the `--cfg`
list, JP skips implicit config loading ([RFD 079]).
Only explicit `--cfg` arguments after `NONE` apply on top of program defaults.

Subsequent `--cfg` arguments must supply any required fields that have no
compiled-in default, such as `assistant.model.id` and
`conversation.tools.defaults.run`.
Otherwise validation fails and reports the missing fields.

#### `WORKSPACE`

`WORKSPACE` resets accumulated config to the workspace's resolved config: the
merged result of implicit loading ([RFD 079]), which is the same baseline new
conversations use by default.

Because implicit loading includes `JP_CFG_*` environment variables, `WORKSPACE`
captures the environment variables in effect at invocation time.
Persisting the reset records those values, so re-running later with different
environment variables does not alter an already-stored reset.

`WORKSPACE` includes whatever value `config_load_paths` (the deferred-loading
search path for `--cfg <name>`) has in the merged workspace config, but does not
load the files in those directories.
Those files are loaded only on an explicit `--cfg <name>` invocation.

#### Entry-local reset with `loader.reset`

A config entry can declare that it resets accumulated config before applying
itself:

```toml
[loader]
reset = "none"
```

`loader.reset = "none"` is the positional equivalent of placing `--cfg=NONE`
immediately before that entry in the `--cfg` stream.
It resets accumulated config to program defaults, then applies the entry's
resolved config, including its own `loader.extends` tree.

```sh
# If `committer` declares `loader.reset = "none"`, these are equivalent
# for config state after the `committer` entry applies:
jp q -c committer
jp q -c NONE -c committer
```

Unlike `--cfg=NONE`, `loader.reset` does not trigger the pre-pipeline gate or
skip implicit config loading, because JP must load the base config before it can
resolve and read the `committer` entry.
Recovering from broken implicit config still requires `--cfg=NONE` or
`--no-cfg`.

JP honors `loader.reset` only when the file is loaded directly as an explicit
`--cfg` entry.
When a file is reached through another file's `loader.extends`, its
`loader.reset` value is ignored so an extended fragment cannot discard its
parent entry's accumulated config.

When a single `--cfg` argument resolves to multiple entries across roots, JP
processes them in root precedence order.
A `loader.reset = "none"` on a later entry resets state at that point,
discarding earlier entries resolved from the same argument.

Only `"none"` is supported; other reset targets can be added if needed.

#### `--no-cfg` shorthand

`--no-cfg` is shorthand for `--cfg=NONE`.
[RFD 070] extends `--no-cfg` to accept a value for targeted revert (`--no-cfg
<source>`, `--no-cfg key=value`), while the bare flag retains its meaning from
this RFD.

### Ordering

All `--cfg` values, including reset keywords, are processed left-to-right ([RFD
008]).
A keyword resets accumulated state at its position, and subsequent `--cfg`
values layer on top.

Because implicit loading happens before the directive loop runs, JP pre-scans
the `--cfg` list for an exact `NONE` match and skips implicit loading when found
([RFD 079]).
Together, the pre-scan and the positional reset discard any directives before
`NONE`:

```sh
# Pre-scan detects NONE → skip implicit loading.
# Directive loop: `foo.toml` applied, then NONE resets to defaults.
# Result: program defaults only.
jp q --cfg=foo.toml --cfg=NONE

# Pre-scan detects NONE. Directive loop: NONE sets state to defaults,
# then foo.toml applied. Result: defaults + foo.toml.
jp q --cfg=NONE --cfg=foo.toml
```

Clap still parses `--cfg` arguments before `NONE` and rejects malformed flags at
parse time (`NONE` recovers from broken implicit config files, not syntax errors
on the command line).
The directive loop then skips loading and merging pre-`NONE` arguments because
`NONE`'s positional reset would discard their effects anyway.

#### Explicit paths under `NONE`

Because `NONE` skips implicit loading, no config files have been read and
`config_load_paths` is empty.
Subsequent `--cfg <name>` directives that rely on search-path resolution fail;
scripts using `NONE` should reference config files by explicit path:

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

#### `NONE` and `WORKSPACE` are mutually exclusive

`--cfg=NONE` and `--cfg=WORKSPACE` cannot appear in the same `--cfg` list.
`NONE`'s pre-pipeline gate skips the implicit loading that `WORKSPACE` expands
to, making the combination contradictory: either `NONE` would be silently
overridden, or `WORKSPACE` would lazily re-run the loading the user opted out
of.
The directive parser rejects the combination before processing any directives,
regardless of position:

```
error: --cfg=NONE and --cfg=WORKSPACE are mutually exclusive.
```

Use `--cfg=WORKSPACE` to reset to the workspace config, or `--cfg=NONE
--cfg=<paths>` to start from program defaults and layer config files explicitly.

### Default behavior

When no reset is present, `--cfg` values layer on top of the implicit starting
config:

- **New conversations** (`--new`): starts from `WORKSPACE`.
- **Continuing conversations**: starts from the stream's current config state.
- **Forked conversations** (`--fork`): out of scope (see
  [Non-Goals](#non-goals)).

A reset keyword at any position replaces the starting config and any preceding
`--cfg` arguments because it sets every field.
Invocations without reset keywords (`jp q` and `jp q --cfg=foo.toml`) behave as
before.

### Disambiguation

`--cfg` checks uppercase keywords by exact string match before any other
resolution:

- `NONE` → keyword
- `WORKSPACE` → keyword
- `none` → file path
- `WORKSPACE.toml` → file path (not an exact keyword match)
- `jp-c17528832001` → conversation ID

For files literally named `NONE` or `WORKSPACE`, force path interpretation with
`@` or a relative path prefix:

```sh
jp q --cfg=@WORKSPACE     # treat as a path
jp q --cfg=./WORKSPACE    # treat as a path (any leading `./` or `../` works)
```

Conversation IDs (`jp-c` prefix plus digits) are out of scope (see
[Non-Goals](#non-goals)), but share the same resolution order: exact keywords
first, then conversation IDs, then file paths, and if a later RFD defines them,
they compose with these keywords in the same pipeline.

### Continuing conversations

When continuing an existing conversation, `NONE`, `WORKSPACE`, and `loader.reset
= "none"` each persist a `ConfigDelta::Reset` event in `events.json` (see
[`ConfigDelta` enum](#configdelta-enum)), followed by `Apply` events for any
state layered on top:

```sh
# Continue conversation, switch to workspace config
jp q --cfg=WORKSPACE

# Continue conversation, reset to program defaults and apply custom config
# (useful for scripts that want predictable state from this point forward)
jp q --cfg=NONE --cfg=mre.toml

```

- **`NONE`** persists `ConfigDelta::Reset` and nothing else from the keyword
  itself.
  Subsequent `--cfg` directives in the same invocation persist as `Apply` events
  after the `Reset` (`[..., Reset, Apply, Apply, ...]`).
- **`loader.reset = "none"`** emits `ConfigDelta::Reset` immediately before the
  resetting entry's `Apply` event, without running the `NONE` pre-pipeline gate.
- **`WORKSPACE`** persists `ConfigDelta::Reset` followed by a
  `ConfigDelta::Apply` carrying the workspace partial resolved at invocation
  time, then any subsequent `--cfg` directives (`[..., Reset, Apply(workspace),
  Apply, Apply, ...]`).
  Because the workspace partial is snapshotted into the `Apply` event, later
  edits to workspace config files do not retroactively change the stored reset;
  run `--cfg=WORKSPACE` again to adopt updated workspace config.

When future invocations fold the stream, `Reset` discards all accumulated config
state and restarts from `PartialAppConfig::default()`, with subsequent `Apply`
events layered on top.
`Reset` events also terminate [RFD 070]'s `-C` walk-back regardless of which
source emitted them.
Chat history (turns, messages, and tool calls) is unaffected.

### `ConfigDelta` enum

To persist resets, `ConfigDelta` is promoted from a struct to an enum:

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
`InternalEvent::ConfigDelta` is wrapped at the `InternalEvent` layer as
`{"type": "config_delta", ...}` (in `crates/jp_conversation/src/stream.rs`), and
`InternalEvent::deserialize` dispatches on that top-level `type` tag.
To avoid colliding with `type`, the discriminator between `Apply` and `Reset`
lives inside the envelope as an `op` field:

```json
// Apply (unchanged from today; no `op` field is written, though an explicit
// "apply" is accepted on read).
{"type": "config_delta", "timestamp": "...", "delta": { /* ... */ }}

// Reset (new).
{"type": "config_delta", "op": "reset", "timestamp": "..."}
```

`Apply` omits `op` on write so its on-disk shape requires no migration.
On read, `deserialize_config_delta` in `crates/jp_conversation/src/stream.rs`
matches `op` strictly: absent (covering all legacy events) or `"apply"` decodes
as `Apply`, `"reset"` decodes as `Reset`, and any other value fails
deserialization.
Rejecting unknown `op` values ensures variants added by newer versions fail
explicitly on older versions instead of being misread as `Apply`.

Fold semantics:

- `Apply`: merge `delta` into accumulated state, apply `unsets`, and record
  claims ([RFD 070]).
- `Reset`: replace accumulated state with `PartialAppConfig::default()` and
  clear per-invocation claims state.
  Subsequent `Apply` events layer on top of defaults.

During stream walk-back ([RFD 070]'s `-C` revert), a `Reset` event terminates
the walk just like reaching the base config, making everything before the
`Reset` unreachable.

### New conversations with `NONE` and `loader.reset`

When creating a new conversation with `NONE` in the `--cfg` list:

- The pre-scan skips implicit loading, so the starting partial is
  `PartialAppConfig::default()`.
- The directive loop merges post-`NONE` directives on top of defaults and writes
  the result to `base_config.json` as a flat partial ([RFD 054]).
- No `Reset` event is emitted at creation time because `base_config.json`
  already represents the post-reset state.
  Subsequent invocations load the conversation from that flat base.

For example:

```sh
jp q -c NONE -c dev --new foobar
```

This writes `base_config.json` with `dev`'s config merged on top of program
defaults, isolating the conversation from later changes to workspace config
files.

`loader.reset = "none"` follows the same creation-time rule at its directive
position.
If `committer` declares `loader.reset = "none"`, then:

```sh
jp q -c committer --new foobar
```

writes the post-reset state to `base_config.json` without emitting a `Reset`
event.

> [!NOTE]
> [RFD 070] reshapes `base_config.json` to `{ base, init }` and adds per-source
> claims.
> Once that lands, post-`NONE` directives at creation time will be stored in
> `init` as `Apply` events with their own claims rather than merged into `base`.
> Until then, fields set by post-`NONE` directives at creation time have no
> per-source provenance.

## Drawbacks

**`--no-cfg` alone fails validation.** Required fields such as `model` have no
compiled-in defaults, so `--no-cfg` without subsequent `--cfg` overrides
produces an invalid config.
This is expected for the escape-hatch workflow, where the user supplies explicit
`--cfg` overrides, so the validation error must clearly report which fields are
missing.

**UPPERCASE flag values are unusual.** Most CLI tools use lowercase flag values,
so uppercase keywords may surprise users initially.
However, uppercase avoids ambiguity with file paths, following the precedent of
Vim's `-u NONE` and `-U NONE`.

**Pre-`NONE` directives are silently dropped.** Because resets are positional,
`--cfg dev --cfg NONE` discards `-c dev`.
Typos or mistaken argument order can therefore lose configuration silently, so
`--cfg` and `--no-cfg` help text should note that `NONE` discards everything
before it.

## Alternatives

### Sigil-prefixed keywords

Use a prefix such as `@` (`--cfg=@parent`, `--cfg=@workspace`) to distinguish
keywords from file paths.
Rejected in favor of uppercase keywords, which require no special characters and
remain visually distinct (while preserving `@` for explicit file paths).

### Lowercase reserved keywords

Use lowercase reserved words (`--cfg=none`, `--cfg=workspace`) and require `@`
or `./` prefixes for files literally named `none` or `workspace`.
Rejected because lowercase tokens are visually indistinguishable from common
file names; uppercase makes keywords obvious and avoids accidental shadowing, as
in Vim's `-u NONE`.

### Separate `--config-base=none|workspace` flag

Express the reset point via a dedicated flag instead of a value within `--cfg`.
Rejected because a separate flag would split the ordered directive stream into
two parallel inputs and break left-to-right composition with other `-c`
directives ([RFD 008]).
Keeping resets inside `--cfg` makes positional sequences like `--cfg=foo.toml
--cfg=NONE --cfg=bar.toml` self-explanatory.

## Non-Goals

- **Conversation-ID inheritance.** `--cfg=jp-c<id>` (expanding another
  conversation's resolved config) and `--fork` implicit config are out of scope.
  They share the `--cfg` disambiguation and directive pipeline established here,
  where inheriting partials behave like any other fully populated source, but
  their fork and inner-conversation provenance rules belong in a future RFD.
- **`START` keyword (reset to conversation creation state).** Expanding a
  keyword to `base + init` from `base_config.json` (the full state the
  conversation was created with) is deferred.
  For most users, `--cfg WORKSPACE --cfg <original-source>` is sufficient, and
  [RFD 070]'s `-C` handles targeted revert of individual sources.
  The `init` list in [RFD 070] preserves the infrastructure needed to add
  `START` in a follow-up RFD.
- **`USER` keyword.** Expanding a `USER` keyword to only the user's global
  config (skipping workspace config) for portable personal defaults across
  projects is deferred and can be added later without design changes.
- **`BASE` keyword.** Expanding a `BASE` keyword to the `base` field of
  `base_config.json` (the creation-time workspace snapshot without `init`
  overrides) is deferred and can be added later without design changes.
- **Config diffing.** Showing what changed between the inherited config and the
  final resolved config is orthogonal.

## Risks and Open Questions

### Validation timing

With `NONE`, config validation must run after all `--cfg` values are applied,
not at base resolution time, because the base config alone is incomplete.

Phase 1 adds a related constraint for continuing conversations:
`add_config_delta` suppresses empty `Apply` diffs by resolving the stream's
current config, and that resolution fails between a `Reset` and whichever
`Apply` restores the required fields.
Phase 2 must append post-reset `Apply` events directly (or defer diffing) rather
than routing them through per-event suppression.

### `NONE` detection ordering

Because `NONE`'s pre-scan gate skips implicit config loading ([RFD 079]), the
CLI entry point must scan `--cfg` values for an exact `NONE` match before
running `load_base_partial` or folding conversation config deltas.
When `NONE` is present, both steps are skipped and the base partial starts at
`PartialAppConfig::default()`, while the conversation stream is still loaded for
chat history.
The positional reset (`ConfigDelta::Reset` for continuing conversations, or
writing post-`NONE` state to `base_config.json` for new conversations) runs
during the subsequent directive loop.

### Reset and `-C` (from RFD 070)

A `Reset` event in the stream terminates [RFD 070]'s `-C` walk-back because
pre-reset state no longer contributes to config resolution.
Running `-C dev` after a `Reset` that discarded `dev`'s claims reports that no
fields are currently claimed by `dev`.

### `loader.reset` metadata timing

`loader.reset` is loader metadata, not persisted application config.
It must be read from a resolved `--cfg` entry before that entry is applied, and
only its reset effect is persisted (through `Reset` events or creation-time base
state), never the `loader.reset` field itself.

## Implementation Plan

### Phase 1: `ConfigDelta` enum and fold-time reset

Promote `ConfigDelta` from a struct to an enum with `Apply` and `Reset` variants
(see [`ConfigDelta` enum](#configdelta-enum)):

- Rename the existing `ConfigDelta` struct to `ApplyDelta` and add `ResetDelta {
  timestamp: DateTime<Utc> }`.
- Define `ConfigDelta` as a plain enum (without `#[serde(tag)]`).
  Hand-roll `Serialize` so `Apply` produces the flat shape without an `op` field
  and `Reset` produces `{"op": "reset", "timestamp": "..."}`, inside the outer
  `InternalEvent` `"type": "config_delta"` wrapper.
- Update `deserialize_config_delta` in `crates/jp_conversation/src/stream.rs` to
  dispatch on `op`: absent or `"apply"` decodes as `Apply`, `"reset"` decodes as
  `Reset`, and any other value returns a deserialization error.
- Update `add_config_delta` in the same file so empty-diff suppression applies
  only to `Apply`; `add_config_delta(Reset)` always appends.
- Centralize stream folding in a `fold_config_delta` helper shared by
  `config()`, `Iter`, `IterMut`, and `IntoIter`:
  - `Apply`: merge `delta` into accumulated state (and apply unsets and claims
    once [RFD 070] lands).
  - `Reset`: replace accumulated state with `PartialAppConfig::default()` (and
    clear per-invocation claims state and terminate `-C` walk-back in [RFD
    070]).
- Add a `timestamp()` accessor on `ConfigDelta`.

Tests:

- `ConfigDelta::Reset` round-trips through `InternalEvent` serialize/deserialize
  with on-disk shape `{"type":"config_delta","op":"reset",...}`.
- `ConfigDelta::Apply` preserves its existing on-disk shape.
- Legacy events without `op` and events with `op == "apply"` both decode as
  `Apply`; unknown `op` values fail deserialization.
- `add_config_delta(ConfigDelta::Reset(_))` always appends, even when an empty
  `Apply` would be suppressed.
- Folding `[Apply(dev), Reset, Apply(fresh)]` resolves to defaults plus `fresh`.
- `-C dev` walk-back after `[Apply(dev), Reset]` reports no matching claims
  (implemented as part of [RFD 070]'s claims work).

Can be merged independently.

### Phase 2: Reset recognition in `--cfg`

Depends on Phase 1.
Add uppercase keyword recognition and entry-local `loader.reset` handling to
`--cfg` processing:

- **Mutual-exclusion check.** Before running any directives, reject `--cfg`
  lists that contain both `NONE` and `WORKSPACE` (see [`NONE` and `WORKSPACE`
  are mutually exclusive](#none-and-workspace-are-mutually-exclusive)).
- **`NONE` pre-pipeline gate.** Scan `--cfg` values for an exact `NONE` match at
  the CLI entry point; when present, skip `load_base_partial` and
  conversation-config folding while still loading the conversation stream for
  chat history.
  Clap still parses pre-`NONE` `--cfg` arguments, and the directive loop skips
  them.
- **`NONE` positional reset.** When the directive loop encounters `NONE`:
  - For new conversations (`--new`, `--fork`), write the post-`NONE` merge
    result to `base_config.json` as a flat partial ([RFD 054]; later reshaped to
    `{ base, init }` by [RFD 070]) without emitting a `Reset` event.
  - For continuing conversations, emit `ConfigDelta::Reset` into `events.json`,
    followed by `Apply` events for post-`NONE` directives.
- **`WORKSPACE` reset.**
  - For new conversations (`--new`, `--fork`), emit no reset events, since new
    conversations already start from workspace state; merge post-`WORKSPACE`
    directives on top.
  - For continuing conversations, emit `ConfigDelta::Reset` followed by
    `ConfigDelta::Apply` with the resolved workspace partial, then `Apply`
    events for post-`WORKSPACE` directives.
- **`loader.reset = "none"`.**
  - Read `loader.reset` from each resolved explicit `--cfg` entry before
    applying its partial, and ignore `loader.reset` on files reached via
    `loader.extends`.
  - For continuing conversations, emit `ConfigDelta::Reset` immediately before
    the entry's `Apply` event.
  - For new conversations, absorb the reset into `base_config.json` just like
    `NONE`.
  - Do not trigger the `NONE` pre-pipeline gate.

Tests:

- Continuing a conversation with `-c committer` (where `committer` sets
  `loader.reset = "none"`) appends `[Reset, Apply(committer)]`.
- Creating a new conversation with `committer` writes the post-reset state to
  `base_config.json` and emits no `Reset` event.
- A file reached through `loader.extends` with `loader.reset = "none"` does not
  reset the parent entry.
- A multi-root `--cfg dev` where a later root's entry sets `loader.reset =
  "none"` discards earlier resolved entries from that same argument.
- `loader.reset = "none"` does not skip implicit loading; broken implicit config
  still fails unless `--cfg=NONE` or `--no-cfg` is used.

## References

- [RFD 008]: Ordered Tool Directives.
  Establishes left-to-right processing for interleaved CLI flags.
- [RFD 079]: Config Sources and Load Order.
  Describes the implicit loading sequence that `NONE` skips and `WORKSPACE`
  re-adopts.
- [RFD 070]: Negative Config Deltas.
  Introduces `init` in `base_config.json` (used by a future `START` keyword),
  extends `ApplyDelta` with claims and unsets, and adds a valued form to
  `--no-cfg` for targeted revert.
- [RFD 054]: Split Conversation Config and Events.
  Defines the flat `base_config.json` format that holds post-`NONE` state until
  [RFD 070] reshapes it.
- `crates/jp_config/src/fs.rs`: config loading and merging logic.
- `crates/jp_cli/src/config_pipeline.rs`: resolved `--cfg` entries and loader
  metadata.
- `crates/jp_conversation/src/stream.rs`: conversation `ConfigDelta` and stream
  fold.
- `crates/jp_config/src/delta.rs`: `PartialConfigDelta` trait and partial-merge
  primitives.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 070]: 070-negative-config-deltas.md
[RFD 079]: 079-config-sources-and-load-order.md
