<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D27: Granular Recovery for Stored Conversation Configs

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-15
- **Summary**: Recovers stored conversation configs at per-field granularity on
  schema mismatches and fills missing required fields from workspace defaults.

## Summary

This RFD updates the conversation configuration compatibility layer to recover
from schema mismatches one field at a time instead of discarding the entire
stored configuration.
Dropping only the incompatible fields keeps the rest of the conversation's
configuration intact, and filling any dropped fields from the workspace
configuration ensures finalization still succeeds.

## Motivation

When the [`AppConfig`] schema evolves between JP versions (a field's type
changes, an enum variant is renamed, or a validator is tightened), conversations
persisted by an older version may contain values that the newer version cannot
deserialize.
The compatibility layer in `jp_conversation::compat::deserialize_partial_config`
has two recovery paths:

1. **Unknown fields** (removed or renamed keys): `strip_unknown_fields` walks
   the schema and removes individual unrecognized keys while preserving the
   surrounding config.
2. **Type mismatches** (changed field types or invalid values): deserialization
   fails and replaces the entire [`PartialAppConfig`] with `empty()`.

Replacing the config with `empty()` locks the user out of the conversation.
For example, changing `DelayDuration` from a `Duration`-backed newtype
(serialized as `{secs, nanos}`) to a humantime string broke every conversation
whose `base_config.json` or `ConfigDelta` set `style.typewriter.*`.
Wiping the stored partial causes `AppConfig::from_partial_with_defaults` to fail
with `StreamError::Config`, because [`AppConfig`] has no `Default`
implementation and requires `assistant.model.id`.
Every command that loads the stream (`jp c show`, `jp c print`, `jp query`) then
fails with `Not found / Conversation events`.

Stored conversation configs should tolerate schema changes automatically,
logging a warning for incompatible fields rather than failing to load the
conversation.

## Design

Recovery works in two stages: first, deserialization drops incompatible fields
from the stored partial one at a time; second, finalization fills any missing
fields from the workspace configuration.

### Scope

Tolerant deserialization applies **only** to data loaded via
`jp_conversation::compat::deserialize_partial_config`:

- `base_config.json` (via `ConversationStream::from_parts`)
- The `delta` subtree of each `ConfigDelta` event in `events.json` (via
  `deserialize_config_delta`)

User-authored configuration sources (`config.toml`, `.jp.toml`, `--cfg` flags,
and environment variables) are loaded through
`jp_config::util::load_partial_at_path` and continue to **error strictly** on
any invalid field.

### User-Facing Behavior

When a stored config contains an incompatible field, JP logs a warning, drops
the field, fills it from the workspace configuration, and loads the
conversation:

```
$ jp c show
WARN Dropped incompatible config field path=style.typewriter.text_delay error=invalid type: map, expected a string
WARN Filled recovered fields from workspace config path=style.typewriter.text_delay

 target  Conversation
     id  jp-c17788309593
  title  Investigate compat layer
  ...
```

### Step 1: Drop Incompatible Fields

Replace the single `from_value` call in `deserialize_partial_config` with a
retry loop that uses [`serde_path_to_error`] to locate and remove the failing
field on each error.

```rust
pub fn deserialize_partial_config(mut value: Value) -> PartialAppConfig {
    let schema = AppConfig::schema();
    let stripped = strip_unknown_fields(&mut value, &schema);
    if stripped > 0 {
        warn!(count = stripped, "Stripped unknown fields from stored config.");
    }

    loop {
        let de = serde_path_to_error::deserialize::<_, PartialAppConfig>(value.clone());
        match de {
            Ok(config) => return config,
            Err(err) => {
                let path = err.path().to_string();
                if path.is_empty() || !remove_at_path(&mut value, &path) {
                    warn!(
                        error = %err.inner(),
                        "Stored config cannot be recovered, replacing with empty config.",
                    );
                    return PartialAppConfig::empty();
                }
                warn!(
                    %path,
                    error = %err.inner(),
                    "Dropped incompatible config field.",
                );
            }
        }
    }
}
```

`remove_at_path` walks dotted paths with bracketed array indices (such as
`providers.llm.aliases[2].name`) and removes the target entry.
If the path is empty or cannot be traversed, `remove_at_path` returns `false`
and `deserialize_partial_config` falls back to `PartialAppConfig::empty()`.

The loop is guaranteed to terminate because each failed iteration removes one
entry from `value`.

### Step 2: Fill Missing Fields from Workspace Config

After Step 1, the recovered partial is well-typed but missing any fields that
were dropped.
If a dropped field is required by [`AppConfig`] and has no
`#[setting(default)]`, finalization would still fail.

To prevent that failure, JP fills missing fields from the workspace's
pre-conversation partial (`pipeline.partial_without_conversation()` in
`jp_cli`), which merges `config.toml`, environment variables, and `--cfg` flags
without per-conversation overlays.
This partial is guaranteed to finalize into a valid [`AppConfig`], because
[`jp`] validates the workspace configuration at startup before loading any
conversation.

Fields resolve in this precedence order, from lowest to highest:

1. Schematic defaults (`PartialAppConfig::default_values`).
2. Workspace partial (`fallback_partial`).
3. Recovered stored partial (output of Step 1).

`PartialConfig::fill_from` copies a field from `other` only when it is `None` in
`self`:

```rust
let effective = stored_partial
    .fill_from(fallback_partial.as_ref().clone())
    .fill_from(default_values);
AppConfig::from_partial(effective, vec![])
```

Finalization cannot fail: every required field without a schema default is
either preserved in the stored partial or supplied by the workspace partial.

### Workspace and Stream Integration

Integrating the fallback partial requires two changes outside `compat.rs`:

1. `Workspace` gains an optional `fallback_partial:
   Option<Arc<PartialAppConfig>>` field with a setter.
   `jp_cli::run_inner` sets it from `pipeline.partial_without_conversation()`
   after Phase 1 of config loading and before any conversation contents are
   accessed.

2. `ConversationStream::from_parts` is split into two steps:

   - A raw build that produces a stream with the recovered stored partial
     attached, without finalizing it.
   - A finalize step that takes a fallback partial and resolves the stored
     partial to an [`AppConfig`].

   The lazy loader in `Workspace::events()` and `metadata()` runs the raw build
   via `LoadBackend::load_conversation_stream`, then finalizes the stream with
   `self.fallback_partial`.

Once finalized, the stream's `base_config: Arc<AppConfig>` behaves like any
other loaded stream, and persistence (`to_parts`) is unchanged.

### Persisting Recovered Configs

Because a dropped field cannot be deserialized, JP cannot preserve or diff the
original value.
Substituting the workspace value and writing the resolved config back on the
next save heals the stored file.
Refusing to persist `base_config.json` after recovery would leave the invalid
bytes on disk, causing every subsequent load to repeat recovery, emit the same
warnings, and depend on whatever workspace configuration is active at the time.

## Drawbacks

- **Repeated deserialization.** The retry loop deserializes the value once per
  dropped field.
  In practice, a stored config rarely contains more than one or two broken
  fields (one in the `DelayDuration` bug), so the overhead is negligible.
- **Transient `--cfg` overrides in the fallback.**
  `pipeline.partial_without_conversation()` includes `--cfg` flags.
  If recovery runs during a command invoked with `--cfg experimental`, that
  override is written into the conversation's `base_config.json` on the next
  save.
  See [Risks and Open Questions].
- **In-memory and on-disk divergence until save.** After recovery, the in-memory
  config differs from `base_config.json` on disk until the conversation is
  persisted again.

## Alternatives

- **Deserialize per top-level field.** Try `from_value` on each top-level field
  of `PartialAppConfig` independently and drop the whole field on failure.
  This is too coarse: an invalid `style.typewriter.text_delay` would discard all
  of `style`.
  The retry loop isolates the failing leaf at comparable cost.
- **Pre-check leaf types against the schema.** Extend `strip_unknown_fields` to
  drop leaves whose JSON type does not match the schematic `SchemaType`.
  This catches primitive type changes cheaply, but it misses semantic errors
  such as unknown enum variants, out-of-range integers, and validator failures.
  It could complement the retry loop later, but cannot replace it.
- **Versioned configs with explicit migrations.** Stamp `base_config.json` and
  each `ConfigDelta` with a schema version and register forward migrations.
  Explicit migrations are auditable, but requiring a migration for every
  breaking change slows iteration while the schema is still changing frequently.
- **Fall back to `AppConfig::default()` on finalize failure.** [`AppConfig`]
  does not implement `Default` because fields like the model ID require user
  configuration.
  Even with synthetic defaults, fallback values would ignore the user's
  configured models, providers, and aliases, whereas the workspace partial
  preserves them.
- **Replace the entire stored config with the workspace partial on any error.**
  This discards valid per-conversation settings alongside the broken field.
  The retry loop preserves every field that still deserializes cleanly.

## Non-Goals

- **Tolerant loading for user config files.** `config.toml`, `.jp.toml`,
  environment variables, and `--cfg` flags continue to error strictly; a typo in
  user-authored config remains a hard failure.
- **Interactive recovery prompts.** A future RFD may introduce a prompt such as
  "this field changed since the conversation was saved: patch this one, patch
  all stored configs, or exit."
  That workflow requires separate design around prompt placement, append-only
  event log updates, non-interactive invocations, and multi-process locking.
- **Forward compatibility for older JP versions.** Reading a newer
  `base_config.json` from an older JP binary is out of scope; this RFD covers
  newer JP versions reading older stored data.
- **Schema versioning infrastructure.** See [Alternatives].

## Risks and Open Questions

- **`--cfg` leakage into stored configs.** The fallback partial includes `--cfg`
  flags.
  If recovery runs with a transient `--cfg` override, those values become
  durable conversation state on the next save.
  A stricter fallback (files and environment variables only, excluding `--cfg`)
  would avoid this at the cost of an extra split in the config pipeline.
  Start with `pipeline.partial_without_conversation()` and split the pipeline if
  this causes issues in practice.
- **Warning fatigue.** A schema change can emit one warning per affected
  conversation on each load until the conversation is saved again, which writes
  the recovered config to disk and silences subsequent loads.
- **`remove_at_path` correctness.** The helper must handle dotted keys and
  bracket-indexed array elements as formatted by `serde_path_to_error::Path`.
  Unit tests must cover nested structs, arrays of structs, and root-level
  failures.
- **Diagnosability of corrupted stored data.** The warning in
  `maybe_init_events` currently omits the underlying load error.
  Logging `error = %err` lets users distinguish schema recovery from file
  corruption.

## Implementation Plan

1. **`serde_path_to_error` retry loop in `jp_conversation::compat`.** Add
   `serde_path_to_error` as a direct dependency of `jp_conversation`, implement
   `remove_at_path(&mut Value, &str)`, and replace the `from_value` branch in
   `deserialize_partial_config` with the retry loop while keeping the empty
   config as the final fallback.
   Add unit tests for type mismatches on optional fields, unknown enum variants,
   nested struct fields, and array elements.
   This step is independently reviewable and mergeable.

2. **Split `ConversationStream::from_parts`.** Introduce `from_parts_partial`
   (returning the recovered `PartialAppConfig` and events) and
   `finalize_with_fallback(self, fallback: &PartialAppConfig)`.
   Keep `from_parts` as a wrapper that calls both with an empty fallback to
   preserve existing behavior for callers without a workspace fallback.
   This step is independently reviewable and mergeable.

3. **Workspace fallback integration.** Add `fallback_partial:
   Option<Arc<PartialAppConfig>>` and a setter to `Workspace`.
   Set it in `jp_cli::run_inner` from `pipeline.partial_without_conversation()`
   after pipeline construction, and update `Workspace::events()` and
   `Workspace::metadata()` to finalize streams via `finalize_with_fallback`.
   Depends on step 2.

4. **Log underlying load errors.** Update `maybe_init_events` and
   `maybe_init_conversation` to include `%error` in their warning logs.
   This step is independent of the others and can merge in any order.

5. **Integration test.** Test the full recovery path: a stored
   `base_config.json` with a type-mismatched required field is filled from the
   workspace partial, the conversation loads cleanly, and the next save writes
   the merged config back to disk.
   Depends on steps 1 through 3.

## References

- [`PartialAppConfig`]
- [`AppConfig`]
- [`serde_path_to_error`]
- [`jp`]

[Alternatives]: #alternatives
[Risks and Open Questions]: #risks-and-open-questions
[`AppConfig`]: ../../../crates/jp_config/src/lib.rs
[`PartialAppConfig`]: ../../../crates/jp_config/src/lib.rs
[`jp`]: ../../../crates/jp_cli
[`serde_path_to_error`]: https://docs.rs/serde_path_to_error
