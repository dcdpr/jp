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

## Summary

Make the stored-conversation config compat layer recover from schema
incompatibilities at field granularity rather than wiping the entire config. A
single invalid field no longer locks the user out of the conversation. Holes
left by recovery are filled from the workspace's currently-resolved config,
guaranteeing that finalize succeeds.

## Motivation

When the [`AppConfig`] schema evolves between JP versions (a field's type
changes, an enum variant is renamed, a validator is tightened), conversations
persisted by the old version may store values that the new version can no
longer deserialize. The current compat layer in
`jp_conversation::compat::deserialize_partial_config` has two recovery paths:

1. **Unknown fields** (field removed or renamed) — granular. The schema-walk
   in `strip_unknown_fields` removes individual keys; surrounding config
   survives.
2. **Type mismatches** (field's type changed) — all-or-nothing. The whole
   [`PartialAppConfig`] is replaced with `empty()`.

The second path is responsible for the lock-out class of bug. A concrete
instance shipped recently: changing `DelayDuration` from a `Duration`-backed
newtype (serialized as `{secs, nanos}`) to a humantime string broke any
conversation whose `base_config.json` or `ConfigDelta` referenced
`style.typewriter.*`. The wipe cascades into a `StreamError::Config` from
`AppConfig::from_partial_with_defaults`, because [`AppConfig`] has no
`Default` impl and requires `assistant.model.id` to be present. The user sees
`Not found / Conversation events` from every command that loads the stream
(`jp c show`, `jp c print`, `jp query`).

We don't want to anticipate which field will break next. We want stored
conversation configs to tolerate any future schema change, surfacing a warning
rather than locking the user out.

## Design

Two independent pieces stack to provide full recovery: a granular per-field
drop pass on the stored partial, and a workspace-config fill pass that
guarantees finalize succeeds.

### Scope

The tolerant path applies **only** to data deserialized via
`jp_conversation::compat::deserialize_partial_config`:

- `base_config.json` (via `ConversationStream::from_parts`)
- The `delta` subtree of each `ConfigDelta` event in `events.json` (via
  `deserialize_config_delta`)

User-authored configuration sources — `config.toml`, `.jp.toml`, `--cfg`
flags, environment variables — are loaded through
`jp_config::util::load_partial_at_path` and continue to **error strictly** on
any invalid field. A typo in the user's config is still a hard failure.

### Recovery, end-user view

```
$ jp c show
WARN Dropped incompatible config field path=style.typewriter.text_delay error=invalid type: map, expected a string
WARN Filled recovered fields from workspace config path=style.typewriter.text_delay

 target  Conversation
     id  jp-c17788309593
  title  Investigate compat layer
  ...
```

The conversation loads, the bad field is dropped, the resulting hole is
filled with the user's current workspace value for that field, and execution
continues.

### Step 1: granular per-field drop

Replace the single `from_value` attempt in `deserialize_partial_config` with a
retry loop that uses [`serde_path_to_error`] to identify and drop the
offending field on each failure.

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

`remove_at_path` is a small helper that walks dotted paths with bracket-index
segments for arrays (`providers.llm.aliases[2].name` and the like). It
returns `false` if the path cannot be navigated (root, or a missing
intermediate), at which point we fall through to the existing empty fallback.

This loop terminates: each iteration either succeeds or removes one field
from the value. The value shrinks monotonically.

### Step 2: fill from the workspace partial

After Step 1 the partial is well-typed but may have holes — fields the user
had set that we had to drop. Some of those fields are required by
[`AppConfig`] and have no `#[setting(default)]`, so finalize would still
fail.

The fix is to fill those holes from the workspace's pre-conversation
partial. The workspace partial (`pipeline.partial_without_conversation()` in
`jp_cli`) is the merge of `config.toml`, environment variables, and
`--cfg` flags — everything *except* per-conversation overlays. By invariant
it must finalize to a valid [`AppConfig`], because [`jp`] itself wouldn't be
running otherwise.

Merge order, weakest to strongest:

1. Schematic defaults (`PartialAppConfig::default_values`).
2. Workspace partial (`fallback_partial`).
3. Recovered stored partial (output of Step 1).

`PartialConfig::fill_from` consumes from `other` only where `self` is `None`,
so the natural expression is:

```rust
let effective = stored_partial
    .fill_from(fallback_partial.as_ref().clone())
    .fill_from(default_values);
AppConfig::from_partial(effective, vec![])
```

Finalize cannot fail: any required-without-defaults field that was in the
stored partial is either still there (Step 1 didn't drop it) or has been
refilled from the workspace partial.

### Plumbing

Two concrete changes outside `compat.rs`:

1. `Workspace` gains an optional `fallback_partial: Option<Arc<PartialAppConfig>>`
   field with a setter. `jp_cli::run_inner` sets it from
   `pipeline.partial_without_conversation()` after Phase 1 of config loading
   and before any conversation contents are accessed.

2. `ConversationStream::from_parts` is split into two halves:

   - A raw build that produces a stream with the recovered stored partial
     (post-Step-1) attached, no finalize.
   - A finalize step that takes a fallback partial and resolves the partial
     to an [`AppConfig`].

   The lazy loader in `Workspace::events()` / `metadata()` runs the raw
   build via the existing `LoadBackend::load_conversation_stream`, then
   finalizes with `self.fallback_partial`.

Once the stream is finalized its `base_config: Arc<AppConfig>` is treated
identically to any other loaded stream. Persistence (`to_parts`) is
unchanged.

### Why persisting the patched config is correct

The field that was dropped is, by construction, unreadable. We have no
representation of the user's original value to compare to or preserve.
Substituting the workspace value is the best information available; writing
it back on the next save is consistent with that substitution. The
alternative — refuse to persist `base_config.json` after recovery — would
leave the unreadable bytes on disk so every subsequent load runs the same
recovery, warns again, and depends on the workspace state of the moment.
That is strictly worse.

## Drawbacks

- Repeated deserialization. Worst-case the retry loop runs once per dropped
  field. In practice the number of broken fields per stored config is small
  (one, in the motivating bug). Negligible cost.
- The fallback uses `pipeline.partial_without_conversation()`, which
  includes `--cfg` flags. A user who runs with `--cfg experimental` while
  recovery fires will bake the experimental layer into the conversation's
  stored `base_config.json` on next save. See [Risks and Open Questions].
- Stored values get harder to inspect once recovered, because the
  in-memory representation no longer matches what's on disk until the next
  persist.

## Alternatives

- **Per-top-level-field deserialize.** Try `from_value` on each top-level
  field of `PartialAppConfig` independently; drop the whole field on
  failure. Generic but coarse — a bad `style.typewriter.text_delay` would
  wipe all of `style`. The retry loop is finer at comparable cost.
- **Schema-aware leaf type pre-check.** Extend `strip_unknown_fields` to
  also drop leaves whose JSON type doesn't match the schematic
  `SchemaType`. Catches the "primitive type changed" subclass cheaply but
  doesn't help with semantic errors (unknown enum variant strings,
  out-of-range integers, validator failures). Useful as a complement but not
  as the only recovery mechanism. Deferred.
- **Versioned configs with explicit migrations.** Stamp
  `base_config.json` and each `ConfigDelta` with a schema version; register
  forward migrations. Explicit and auditable but turns every breaking change
  into a deliberate migration step. Right answer eventually, wrong answer
  for the project's current iteration speed. Deferred.
- **Fall back to `AppConfig::default()` on finalize failure.** There is no
  `Default` impl on [`AppConfig`] because some fields (model id) require
  user configuration. Even if one existed, defaults wouldn't reflect the
  user's chosen models, providers, or aliases — the workspace partial does.
- **Replace the entire stored config with the workspace partial on any
  error.** Discards user intent for fields that *were* recoverable. The
  retry loop preserves the parts that still parse.

## Non-Goals

- **No change to user config file loading.** `config.toml`, `.jp.toml`,
  environment variables, and `--cfg` flags continue to error strictly. A
  typo in user-authored config is still a hard failure.
- **No interactive recovery prompt.** A future RFD may introduce a flow
  like "this field changed since the conversation was saved — patch this
  one, patch all stored configs, or exit." That feature has its own design
  surface (where the prompt fires, semantics of "patch all" against the
  append-only event log, non-interactive invocations, multi-process locking)
  and is deferred.
- **No reverse compatibility.** An older JP reading a newer
  `base_config.json` is out of scope. This RFD covers newer JP reading
  older stored data.
- **No schema versioning infrastructure.** See [Alternatives].

## Risks and Open Questions

- **`--cfg` leakage into stored configs.** The fallback partial includes
  `--cfg` flags. If recovery fires during a run with a transient `--cfg`,
  those values become durable conversation state on the next save. A
  stricter fallback (files + env only, no `--cfg`) would avoid this at the
  cost of an additional split in the config pipeline. Ship the simpler
  version first; revisit if it bites.
- **Warning fatigue.** A widely-shipped schema change can produce one
  warning per affected conversation per load. Mitigated by the fact that
  the next save flushes the recovered config back to disk, removing the
  warning on subsequent loads.
- **`remove_at_path` correctness.** The helper has to handle dotted keys
  and bracket-indexed array elements as produced by
  `serde_path_to_error::Path`. Test coverage needs to include nested
  structs, arrays of structs, and the root case.
- **Diagnosability of corrupted stored data.** The current
  `maybe_init_events` warning drops the underlying error. We should also
  surface the actual load error (`error = %err`) so users can tell
  recovery from genuine corruption. Small fix, included in the plan.

## Implementation Plan

1. **`serde_path_to_error` retry loop in `jp_conversation::compat`.** Add
   the crate as a direct dependency of `jp_conversation`. Implement
   `remove_at_path(&mut Value, &str)`. Replace the `from_value` branch in
   `deserialize_partial_config` with the retry loop. Keep the empty fallback
   as the last-ditch path. Tests: type mismatch on optional field,
   semantic error (unknown enum variant), nested struct field, array
   element. Reviewable and mergeable independently — already a strict
   improvement over the current behavior.

2. **Split `ConversationStream::from_parts`.** Introduce
   `from_parts_partial` (returns the recovered
   `PartialAppConfig` and the events) and `finalize_with_fallback(self,
   fallback: &PartialAppConfig)`. Existing `from_parts` becomes a thin
   convenience wrapper that calls both with an empty fallback (preserving
   current behavior). Reviewable and mergeable independently.

3. **Workspace fallback plumbing.** Add
   `fallback_partial: Option<Arc<PartialAppConfig>>` to `Workspace` with a
   setter. `jp_cli::run_inner` sets it from
   `pipeline.partial_without_conversation()` after pipeline construction.
   `Workspace::events()` and `Workspace::metadata()` finalize via
   `finalize_with_fallback` instead of the empty path. Depends on (2).

4. **Diagnostic warning improvements.** Update `maybe_init_events` and
   `maybe_init_conversation` to log the underlying load error
   (`%error`). Independent of the rest; can ship in any order.

5. **Integration test.** Exercise the full path: stored
   `base_config.json` with a type-mismatched required field, workspace
   partial supplies the replacement, conversation loads successfully, next
   persist writes the merged config back. Depends on (1)–(3).

## References

- [`PartialAppConfig`]
- [`AppConfig`]
- [`serde_path_to_error`]
- [`jp`]

[`AppConfig`]: ../../crates/jp_config/src/lib.rs
[`PartialAppConfig`]: ../../crates/jp_config/src/lib.rs
[`jp`]: ../../crates/jp_cli
[`serde_path_to_error`]: https://docs.rs/serde_path_to_error
[Risks and Open Questions]: #risks-and-open-questions
[Alternatives]: #alternatives
