# RFD 070: Negative Config Deltas

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-03
- **Required by**: [RFD 078]

## Summary

This RFD introduces `-C` / `--no-cfg` as the negative counterpart of `-c` /
`--cfg`.
A negative config argument accepts the same inputs as `-c` and reverts the
matching config's influence on the conversation.
To support precise per-source revert, each `ConfigDelta` gains a **claims map**
that records which config source last set each field.
All forms of `-C` (file, key-value, JSON object, and shortcut flag) use the same
**claim-history-driven** revert: the scope of each `-C` is derived from the
conversation's claims, not from the current contents of the named source.
Sources are distinguished via a kv-style identity for non-file overrides.

## Motivation

Today a user can layer config files onto a conversation:

```sh
jp query -n -c dev       # new conversation with dev overrides
jp query -c architect    # add architect overrides on top
```

There is no way to *remove* a previously applied config's influence.
If the user wants to stop using `dev` without starting a fresh conversation,
they must manually identify every field `dev` set and override each one with
`--cfg key=value`.
This is tedious and error-prone.

The expected workflow is:

```sh
jp query -C dev          # "undo" dev's overrides
```

This should revert fields that `dev` introduced, but leave untouched any field
that was subsequently claimed by another config source (e.g.
`architect`).
If both `dev` and `architect` set `tools = [read_file]`, reverting `dev` should
not disable the tool — `architect` still wants it.

If we do nothing, users must either track config state manually or start new
conversations when they want to change config profiles.
Neither is good UX, and the latter is destructive.

## Design

### CLI surface

Add `-C` / `--no-cfg` as a global flag.
It accepts the same input syntax as `-c` / `--cfg` — config file paths,
`key=value` assignments, and JSON object literals:

```sh
jp query -C dev                                    # revert a file
jp query -C dev -c architect                       # revert dev, apply architect
jp query -C dev -C debug                           # revert both files
jp query -C assistant.name=JP                      # revert a single field
jp query -C '{"assistant":{"name":"JP"}}'          # revert via JSON object
jp query -C dev -C assistant.model.id=anthropic    # mix file and key-value
```

Anything that `-c` can convert into a `PartialAppConfig`, `-C` can use as a
revert target.

All forms of `-C` are claim-history driven, but they pick their revert scope
differently depending on the form:

- **File-based** (`-C dev`): **scopes by source identity**.
  The source identity is the file's [`id`](#stable-identity-via-id) hash (or
  resolved-path hash if `id` is absent).
  Multi-root resolution ([RFD 035]) produces a set of hashes, one per resolved
  file.
  Only each file's top-level `id` field is read from disk to determine identity;
  the full partial is never loaded for revert scope.
- **Key-value** (`-C foo=BAR`): **scopes by current value**.
  `-C` reverts `foo` only when the field's current resolved value equals `BAR`,
  and walks back past all claims on `foo` (regardless of source) to find the
  prior different state.
  This matches the user intent "undo `foo` being `BAR`."
  See [Revert algorithm](#revert-algorithm).
- **JSON object** (`-C '{...}'`): pre-expanded via `try_merge_object` (the same
  path used by `-c` for JSON input) into per-leaf kv reverts.
  Each leaf runs the value-based kv revert independently.
- **Shortcut flags** don't have a `-C` form of their own.
  To revert a shortcut flag's effect, use the equivalent key-value form (`-C
  assistant.model.id=foo` to undo `--model foo`).

**File-based scope comes from claim history, not from the current file.** `-C
dev` reverts fields that `dev` claimed in this conversation's history, even if
`dev.toml` has since been edited to remove those fields.

A claim carries one or more identity hashes per source.
A workspace file's claim always includes the workspace-relative path hash, and
additionally includes `hash(id)` if the file declares one — either is enough to
match a target at revert time.
This means workspace files are always targetable by `-C` even after deletion,
with or without `id`.
User-local and user-workspace files use scan-based resolution at revert time;
they become unreachable post-deletion.
See the [Source identity](#source-identity) section for the full per-base
breakdown.

### Processing model

Negative args are processed left-to-right together with positive args, following
the ordered-directive model from [RFD 008].
A `-C` / `--no-cfg` at any position in the `--cfg`/`--no-cfg` sequence operates
on whatever the accumulated state is at that point.

```sh
# Left-to-right: apply dev, then revert dev (net effect: no dev)
jp query -c dev -C dev

# Left-to-right: revert dev first (no-op if not claimed), then apply architect
jp query -C dev -c architect
```

#### Phase boundaries

JP's config pipeline runs in two phases (see [RFD 079]):

- **Phase 1** resolves `conversation.default_id` without a conversation loaded.
  `partial_without_conversation()` processes `-c` directives to extract the
  default conversation ID, before a conversation stream is available.
- **Phase 2** runs after the conversation is loaded.
  The full pipeline, including the per-conversation layer, is applied.

`-C` requires a conversation's claim history to compute its scope, so it is
**phase 2 only**: phase 1 skips every `CfgDirective::Revert` entry and only
processes `Apply` directives to resolve `conversation.default_id`.
This means `-C` cannot influence which conversation is loaded.

In practice, `conversation.default_id` is set via workspace config or session
state, not via mid-sequence `-C`, so this limitation is not user-facing.

### Data model changes

#### `CfgDirective` wrapper

`KeyValueOrPath` is unchanged.
A new wrapper enum captures whether a config arg is additive or subtractive:

```rust
/// A config layer directive: apply or revert.
enum CfgDirective {
    /// Merge this config on top of the current state.
    Apply(KeyValueOrPath),
    /// Revert fields that match this config.
    Revert(KeyValueOrPath),
}
```

`-c` args produce `CfgDirective::Apply`, `-C` args produce
`CfgDirective::Revert`.
Both share the same `KeyValueOrPath` resolution logic.
The interleaved sequence preserves left-to-right ordering.

`ResolvedCfgArg` changes its `Partials` variant to carry source metadata
alongside each partial, so the claims pipeline knows which file contributed each
field:

```rust
enum ResolvedCfgArg {
    KeyValue(KvAssignment),
    Partials(Vec<(PartialAppConfig, SourceId)>),
}
```

A new wrapper carries the polarity through resolution:

```rust
enum ResolvedCfgDirective {
    Apply(ResolvedCfgArg),
    Revert(ResolvedCfgArg),
}
```

The polarity (apply vs. revert) is orthogonal to the resolution type (key-value
vs. file partials).
`apply_cfg_args` matches the outer enum for polarity and the inner for
resolution type:

- `Apply(*)` — merge as today, plus record claims (file identity for
  `Partials`, kv identity for `KeyValue`).
- `Revert(Partials(...))` — file-based, **identity-scoped**: revert fields
  whose current claim is in the target file identity set.
- `Revert(KeyValue(...))` — key-value, **value-scoped**: revert the named field
  if its current resolved value matches the specified value, walking back past
  all claims on that field.

#### Claims on `ConfigDelta`

`ConfigDelta` (today a struct in `crates/jp_conversation/src/stream.rs`) gains
two new fields recording per-field provenance and explicit field clearing:

```rust
pub struct ConfigDelta {
    pub timestamp: DateTime<Utc>,
    pub delta: Box<PartialAppConfig>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsets: Vec<String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub claims: BTreeMap<String, Vec<String>>,
}
```

- **`delta`**: the config diff, same as today.

- **`unsets`**: dotted field paths to reset to `None`.
  Populated by revert deltas (always — for every field in the revert's scope)
  and optionally by normal deltas that want to clear a field.
  Applied in `apply_config_delta` **before merging the delta's partial**: each
  field in `unsets` is reset to `None`, then the delta's partial is merged on
  top.
  This ordering gives revert deltas Replace-equivalent semantics for custom
  merge types without needing per-type handling (see [Revert semantics for
  custom merge types](#revert-semantics-for-custom-merge-types)).

  Implementation: add an `unset(path: &str)` method to `PartialAppConfig` (and
  nested partial types) that mirrors the existing `AssignKeyValue` dispatch but
  sets the target field to `None` instead of assigning a value.
  For vec-element unsets on set-like and identity-bearing vec fields (see [Claim
  granularity](#claim-granularity)), a `remove_element(path: &str, identity:
  &str)` method filters the target array by matching each element's
  `ClaimIdentity` against the stored identity.
  Duplicate-capable vec fields do not use `remove_element`; they use whole-field
  revert via `unset(path)` plus a direct assignment of the prior value.
  All methods operate at the Rust type level, avoiding JSON serialization
  round-trips that would be fragile across custom serde implementations (e.g.
  `MergeableVec`, `MergedString`).

- **`claims`**: field path → list of `"HASH:LABEL"` entries.
  Three states distinguish:

  - **Field absent from the map** — no provenance (this delta didn't claim the
    field).
  - **Field present with an empty `Vec`** — explicitly unclaimed, reserved for
    environment variable overrides (see [Key-value and shortcut flag
    claims](#key-value-and-shortcut-flag-claims)).
  - **Field present with one or more entries** — claimed.
    A file may carry multiple identity hashes (e.g.
    `hash(id)` *and* `hash(path)` for a workspace file that declares `id`),
    letting revert match on any one of them.
    Matching uses the `HASH` prefix of each entry; the `LABEL` is for display
    only.

#### Source identity

Each claim source is stored as a `HASH:LABEL` string.
The hash (e.g.
SHA-256) is always present for identity matching.
The label provides human-readable context for provenance display (e.g.
[RFD 060]) but varies by source location to avoid leaking user-specific paths
into shared workspace storage:

| Source type                     | Label                      | Example                            |
| ------------------------------- | -------------------------- | ---------------------------------- |
| File with `id` field            | The `id` value             | `a1b2c3:dev-persona`               |
| Workspace file (no `id`)        | Workspace-relative path    | `d4e5f6:.jp/config/skill/dev.toml` |
| User-workspace file             | `<user-workspace>`         | `a1b2c3:<user-workspace>`          |
| User-local file                 | `<user-local>`             | `d4e5f6:<user-local>`              |
| Structured object with `id`     | The `id` value             | `f7a8b9:quick-model`               |
| Conversation ID                 | The conversation ID        | `c0d1e2:jp-c17528832001`           |
| Key-value assignment            | The canonical field path   | `11a2b3:assistant.model.id`        |
| Shortcut flag (`--model`, etc.) | Same as kv (maps to field) | `11a2b3:assistant.model.id`        |
| Environment variable            | —                          | Explicit unclaim (empty `Vec`)     |

A single source can contribute **multiple identity hashes** to a claim, and a
claim matches a revert target if any of its stored hashes appears in the target
set.
Per-base hashing rules:

- **Workspace file**: always hash the workspace-relative path (e.g.
  `.jp/config/skill/dev.toml`).
  If the file declares an `id`, additionally hash the `id` value.
  The path hash is edit-stable and derivable without the file, so workspace
  files remain targetable by `-C` after deletion.
- **User-workspace / user-local file**: hash the absolute resolved path.
  If the file declares an `id`, additionally hash the `id` value.
  Revert relies on scanning the base directory to discover candidate files; a
  deleted file isn't found by scan and can't contribute a candidate hash.

Renaming a file without `id` changes its path-based identity — the `id` hash
provides stable cross-rename identity, which is why sources that expect to be
renamed should declare `id`.

Conversation streams are part of the workspace and typically shared via VCS.
Workspace-relative paths are safe to store verbatim.
User-workspace and user-local paths are redacted to placeholders since they may
reveal personal directory structure.
The hash is always available as a fallback — `config explain` ([RFD 060]) can
attempt to resolve a placeholder by hashing all known config files and matching
against the stored hash.

#### Stable identity via `id`

Config files can declare an optional `id` field for stable identity:

```toml
# .jp/config/skill/dev.toml
id = "dev-persona"

[assistant]
name = "DevBot"
```

When present, the `id` is used instead of the file path for claim matching.
This survives file renames: if `dev.toml` is renamed to `developer.toml` but
keeps `id = "dev-cfg"`, `-C developer` still matches claims from the old file.

Two files with the same `id` are treated as the same config identity.
This is intentional — a team-shared config and a personal override with the
same `id` can replace each other without breaking claims.

The `id` field is added to `AppConfig` as an optional field, similar to the
existing `inherit` field.
The `schematic` `#[derive(Config)]` macro auto-generates the corresponding
`Option<String>` slot on `PartialAppConfig`.
It is read during `--cfg` resolution to compute source identity, and stripped
(set to `None`) during merging so it does not appear in the resolved `AppConfig`
or in persisted config deltas.

### Claims lifecycle

#### Building claims during `-c`

When the pipeline processes a `-c` arg, it merges the partial into the
accumulated config AND records claims for each field the partial sets:

```txt
claims = {}

for arg in cfg_args:
    if Apply(partials):   // one or more files from multi-root resolution
        for file in partials:
            partial = merge(partial, file.partial)
            identities = file.source_identities()  // ["HASH:LABEL", ...]
            for path in set_field_paths(&file.partial):
                claims[path] = identities.clone()

    if Apply(kv):
        partial = assign(partial, kv)
        identity = kv_identity(&kv.field_path, &kv.canonical_value)
        claims[kv.field_path] = vec![identity]
```

Key-value assignments record their own claim under a distinct kv-style source
identity.
Kv `-C` does **not** use this identity for matching (see [Key-value
revert](#key-value-revert-c-foobar)); the identity's sole purpose is to let a
subsequent file-based `-C <source>` recognize that the field is owned by an
explicit user override and skip it.
See [Key-value and shortcut flag claims](#key-value-and-shortcut-flag-claims)
below for the identity computation.

A single `-c dev` can resolve to multiple files across config roots ([RFD 035]):
user-global, workspace, and user-workspace.
Each file gets its own source identity and claims.
Files are merged in precedence order (user-global \< workspace \<
user-workspace), so if two files set the same field, the higher-precedence
file's claim overwrites the lower one.

If multiple files share the same `id` value, they hash to the same source
identity.
Claims from all of them are attributed to that single identity.
This is intentional — all instances represent "the same config source"
regardless of which root they came from.

Within a single invocation, later `-c` args overwrite earlier claims for the
same field.
This matches left-to-right merge semantics.

#### Key-value and shortcut flag claims

Both `-c key=value` and CLI shortcut flags (`--model`, `--reasoning`, etc.)
record claims with a uniform kv-style source identity:

```txt
kv_identity(field_path, canonical_value) =
    hash("kv:" + field_path + "=" + canonical_value)
```

The canonical value is the field's typed value re-serialized via its standard
serde form, with any transformations (e.g. model alias resolution) already
applied.
So `--model opus` (where `opus` resolves to `anthropic/claude-opus-4-6`) and `-c
assistant.model.id=anthropic/claude-opus-4-6` produce the same identity
`hash("kv:assistant.model.id=anthropic/claude-opus-4-6")`.

This identity is used **only for apply-side claim recording**, not for `-C`
matching.
Kv `-C foo=BAR` is value-based (see [Key-value
revert](#key-value-revert-c-foobar)).
The kv identity still matters because it appears in the claims state, where it
tells a subsequent `-C <source>` that the field is owned by an explicit user
override rather than by the named source — so `-C dev` won't silently revert a
field that was later pinned by `-c assistant.model.id=something`.

Shortcut flags record claims by threading a `claims: &mut BTreeMap<String,
Vec<String>>` through `IntoPartialAppConfig::apply_cli_config` and each
`apply_*` helper.
Each helper registers the `(field_path, canonical_value)` pair alongside its
partial mutation:

```rust
fn apply_model(
    partial: &mut PartialAppConfig,
    model: Option<&str>,
    providers: &ProvidersConfig,   // for alias resolution
    claims: &mut BTreeMap<String, Vec<String>>,
) {
    let Some(id) = model else { return };
    let resolved = providers.llm.aliases.resolve(id).to_string();
    partial.assistant.model.id = resolved.clone().into();
    claims.insert(
        "assistant.model.id".into(),
        vec![kv_identity("assistant.model.id", &resolved)],
    );
}
```

The `&'static str` field path lives next to the assignment — the only place
that can stay correct as the code evolves.
A static flag-to-field mapping table was rejected because several helpers do
non-trivial work (`apply_editor` branches on variants; `apply_reasoning` touches
multiple fields), which a table would not capture.

Environment variable overrides (`JP_CFG_*`) are the one principled exception:
they record `claims[path] = vec![]` (empty vec = explicit unclaim).
The rationale is lifecycle, not technical — env vars are ambient session state,
not per-invocation intent.
A user who wants to undo an env override removes it from the environment; there
is no `-C env:...` syntax to invent.
The empty entry also prevents a later file-based `-C source` from silently
reverting an env-set field.

#### Persisting claims: per-directive deltas

This RFD changes the persistence model from **one `ConfigDelta` per invocation**
to **one per apply step**:

- Each `-c` directive (file or key-value) emits its own delta carrying just that
  directive's contribution and claims.
- Each `-C` directive emits its own delta carrying the revert (which fields were
  unclaimed, what the reverted values are, plus any `unsets`).
- All CLI shortcut flags (`--model`, `--reasoning`, etc.) batch into a **single
  trailing delta** after all `-c`/`-C` directives.
  Their kv identities are distinct (one per touched field), so batching is safe
  — no two flags claim the same field under different identities within one
  batch.

A `ConfigDelta` is emitted whenever the diff OR the claims map OR `unsets` is
non-empty.
This is a change from the current behavior, which skips empty diffs — a
claims-only delta (e.g. when `-c architect` sets a field to the same value `dev`
already set) must still be stored to update provenance.

**Why per-directive.** Consider a common workflow:

```sh
jp query -c dev -c architect       # invocation 1: layer dev then architect
jp query -C architect              # invocation 2: drop architect
```

Under one-delta-per-invocation (today's model), the invocation 1 delta's claims
map would reflect only the final state — architect wins every field it and dev
both touched.
The intermediate dev claim is lost.
In invocation 2, `-C architect` walking back past architect's claim reaches only
the base config for shared fields, not dev's values.
Dev's influence on shared fields is silently erased.

With per-directive deltas, invocation 1 persists two deltas: delta A (dev's
contributions and claims) then delta B (architect's contributions and claims).
In invocation 2, walking back past architect's claim lands on dev's, and dev's
values re-emerge.
The user's "drop architect, keep dev" intent holds regardless of whether dev and
architect were applied in the same or separate invocations.

The cost is one persisted event per directive instead of per invocation — see
[Drawbacks](#drawbacks).

**Legacy backward-compat.** Old conversation streams without claims deserialize
with an empty map (`#[serde(default)]`).
For fields with no claim entry, `-C` skips — there is no provenance to match
against, and guessing from diff values would produce unpredictable results.
`-C` therefore only works precisely on conversations created after the claims
feature lands; legacy conversations are unaffected.

#### Delta plumbing changes

`ConfigDelta` is currently handled by hand-rolled serde and helper functions
that drop unknown fields.
Adding `claims` and `unsets` to the struct with `#[serde(default)]` is **not**
sufficient on its own.
Three concrete changes are required:

- **`deserialize_config_delta`** (`crates/jp_conversation/src/stream.rs`, line
  ~158) currently reads only `delta` and `timestamp` from a raw
  `serde_json::Value`.
  It must be extended to read `claims` and `unsets` explicitly.
  The hand-rolled function bypasses derived deserialization, so struct-level
  `#[serde(default)]` never runs.
- **`add_config_delta`** (line ~302) currently destructures `{ delta, timestamp
  }` and skips emitting when the recomputed diff is empty.
  It must:
  - destructure and preserve `claims` and `unsets` through the
    diff-recomputation step,
  - emit the delta when `claims` or `unsets` is non-empty, even if the value
    diff is empty (claims-only deltas carry provenance that cross-invocation
    `-C` depends on).
- **Single `apply_config_delta` helper.** Delta replay happens in six places
  today: `config()`, `IntoIter::next` and `next_back`, `Iter::next` and
  `next_back`, `IterMut::next`.
  Each site currently calls `partial.merge(&(), delta.into())` directly.
  Introduce a single `apply_config_delta(partial: &mut PartialAppConfig, delta:
  &ConfigDelta)` helper that **applies `delta.unsets` first, then merges
  `delta.delta`**, and replace all six call sites with it.
  The unset-before-merge order is load-bearing for revert semantics (see [Revert
  semantics for custom merge types](#revert-semantics-for-custom-merge-types))
  and ensures `config()` and the per-event iterators produce identical views.

#### Current claims state

Before running an invocation's directive loop, the pipeline builds the **current
claims state**: a `BTreeMap<String, Vec<String>>` recording the identities that
claim each field (absent = no claim, empty vec = explicitly unclaimed, non-empty
= claimed on any listed identity).
This state is what the `-C` revert algorithm consults to compute scope.
It is built in order:

1. **Persisted claim history fold.** Walk the `base` partial's empty claims →
   each `init` `ConfigDelta` entry in `base_config.json` (the creation-time
   per-directive deltas) → each event-stream `ConfigDelta` in `events.json`.
   Take the most recent claim per field.
2. **Current-invocation env unclaims.** For each field set in the **env-only
   partial** — the result of `PartialAppConfig::from_envs()` in
   `crates/jp_config/src/lib.rs` — overwrite the claims state with `vec![]`
   (empty vec = explicit unclaim).
   Note that `load_envs()` (same file, `util.rs`) returns the base **merged
   with** env, which is unsuitable here: we need the env-only set of touched
   fields to know which entries to unclaim.
   This step protects env-set fields from a subsequent `CfgDirective::Revert`
   that might otherwise walk back to a pre-env persisted claim and emit a delta
   that overrides the live env value.

The env-seeding step matters because env vars are applied before the
`ConfigPipeline` runs (`load_base_partial` in `crates/jp_cli/src/lib.rs` calls
`load_envs` to merge env into the base partial before the pipeline sees any
directive), but their unclaim effect must be present in the claims state when
phase 2 processes `-C`.
Without this step, a user with `JP_CFG_ASSISTANT_MODEL_ID=...` set could run `jp
query -C dev` and have the env-set model overwritten by a revert to dev's prior
value.

During left-to-right `-c`/`-C` processing:

- Each `Apply` updates the claims state with its contributed claims.
- Each `Revert` updates each in-scope field to its **post-revert claim**: the
  prior non-target claim found during walk-back, or no claim (absent from the
  map) if the walk reached the base config.
  This matches the emitted delta's `claims` map exactly, so later directives in
  the same invocation see the correct current owner.
  For example, after `-c dev -c architect -C architect` processes left-to-right,
  `-C dev` following in the same invocation sees fields owned by dev (restored
  by the architect revert) and can undo them.

Each directive's claims delta is persisted (see [Persisting
claims](#persisting-claims-per-directive-deltas)) rather than being squashed
into an invocation-level map.

### Revert algorithm

`-C` has two scoping modes depending on the flag's form: **file-based reverts
scope by source identity**, **key-value reverts scope by current value**.
Both are driven by the conversation's claim history but answer different
questions.
JSON-object `-C` is just a fan-out into multiple kv reverts.

#### File-based revert: `-C <source>`

1. **Compute the target identity set.**

   - Resolve `<source>` via `config_load_paths` across roots ([RFD 035]) to one
     or more candidate paths (existing or not).
   - For each candidate, compute every applicable identity hash:
     - If the file exists and declares a top-level `id`: include `hash(id)`.
     - Workspace files: include `hash(workspace-relative path)` — derivable
       without the file (path is edit-stable), so this works even when the file
       is deleted.
     - User-local / user-workspace files: include `hash(absolute resolved path)`
       — requires the file to exist because resolution is scan-based, not
       path-derivable.
       Each candidate contributes 1 or 2 hashes; files with `id` and known path
       contribute both.
   - The target set is the union of all computed hashes.
     Files sharing the same `id` collapse on that hash; path-based identities
     stay distinct per resolved path.

2. **Determine field scope from the current claims state** (built per [Current
   claims state](#current-claims-state), including env unclaims).

   Scope is `{field | claims_state[field] has any identity in target_set}`.
   Purely claim-history driven — the source's current file contents are never
   consulted for scope.

   Claims state entries:

   - **Non-empty `Vec`** with any entry whose hash is in the target set →
     **target owns it**, include in scope.
   - **Non-empty `Vec`** with no entry in the target set → **another source
     owns it**, skip.
   - **Empty `Vec`** → **explicitly unclaimed** (env var override), skip.
   - Field not in `claims_state` → **no provenance data**, skip.

3. **Compute revert values.** For each field in scope, walk `ConfigDelta` events
   backwards past all claims that match the target set (any stored identity in
   the target set), until a claim with no target-matching identity or the base
   config is reached.
   Use the config value at that point.
   If the target value is `None`, add the field path to the revert delta's
   `unsets` list instead of its partial (schematic's merge cannot express `Some
   → None` transitions).

4. **Emit** a new `ConfigDelta` representing the revert itself: its `delta`
   carries the reverted values, `unsets` carries any unset paths, and its
   `claims` map records each reverted field's new owner (the prior non-target
   claim, or absent if revert reached base).

#### Key-value revert: `-C foo=BAR`

1. **Read the current resolved value** of `foo` from the pipeline's current
   state (after persisted deltas and env have been folded).
2. **Compare.** If the current value does not equal `canonical(BAR)`, skip with
   a diagnostic — the user asked to undo a value that isn't currently set.
3. **Walk back.** Starting from the most recent `ConfigDelta` in the stream,
   walk backward through events, looking for the first delta whose application
   yielded a `foo` value different from `BAR`.
   All claims on `foo` between that point and the present are considered part of
   the "assignment to be undone," regardless of which source owns each.
4. **Emit** a `ConfigDelta` that reverts `foo` to that earlier different value
   (or `unsets[foo]` if it was unset), with `claims` reflecting the new owner of
   `foo` (the claim at the earlier point, or absent if reached base).

Consequence: `-C foo=BAR` undoes `foo=BAR` regardless of who set it.
The last `-c dev` that set `foo=BAR`, the kv assignment `-c foo=BAR`, a shortcut
flag batch, or any combination — all are undone if the current value is `BAR`.
The user's "undo this value" intent matches behavior.

#### JSON-object revert: `-C '{...}'`

Pre-expand the JSON via `try_merge_object` into per-leaf assignments, then run
the key-value revert algorithm independently for each leaf.
Mismatched values on some leaves produce per-leaf diagnostics; matches revert
normally.
The overall command succeeds even if only some leaves match.

#### Why claim-history, not current-file-contents

Deriving file-revert scope from the current contents of the target file would
miss fields that the file no longer sets after being edited — leaving stale
values behind.
Claim-history scope reflects historical intent ("undo this source's influence on
this conversation") rather than current-file intent ("undo whatever this file
says now").

The approach handles file edits cleanly.
Deletion support depends on base:

- **Workspace files**: always targetable post-deletion.
  The workspace-relative path is edit-stable and always hashed into the claim,
  so the target identity set can be reconstructed without the file.
  This holds whether or not the file declared `id`.
- **User-local / user-workspace files**: not targetable post-deletion.
  Their identities are absolute paths derived via scan-based resolution at
  revert time; a deleted file isn't found by scan, so no candidate hash is
  produced.
  If stable revert is needed across deletion, declare `id` on the file early and
  keep it present, OR move it into the workspace so the path-derived identity
  survives deletion.

See also [RFD 060] for the provenance-display angle.

#### Revert semantics for custom merge types

The config has several fields backed by custom merge types whose strategies can
be Append, Prepend, or Replace:

- `MergeableString` (e.g.
  `assistant.system_prompt`, see `crates/jp_config/src/types/string.rs` and
  `internal/merge/string.rs`).
- `MergeableVec` (e.g.
  `assistant.instructions`, `conversation.attachments`, see
  `crates/jp_config/src/types/vec.rs` and `internal/merge/vec.rs`).
- `MergeableMap` (e.g.
  `plugins.command`, `providers.mcp`, see `crates/jp_config/src/types/map.rs`
  and `internal/merge/map.rs`).

A revert delta that encoded its target value with, say, Append strategy would
concatenate the target onto the current resolved state rather than replacing it.
The RFD's design avoids this class of bug entirely by leaning on `schematic`'s
Option-handling at the partial-merge layer.

**The mechanism**: every revert delta writes the field path in `unsets` **and**
the target value in `delta.delta`.
The `apply_config_delta` helper applies unsets first (field → `None`), then
merges `delta.delta` on top.
Schematic's `merge_setting(prev: Option<T>, next: Option<T>, ...)` in
`crates/schematic/src/internal.rs` gates on both sides being `Some`:

```rust
if prev.is_some() && next.is_some() {
    merger(prev.unwrap(), next.unwrap(), context)
} else if next.is_some() {
    Ok(next)
} else {
    Ok(prev)
}
```

When `prev = None` and `next = Some(target)`, the custom `merger` (e.g.
`string_with_strategy`, `vec_with_strategy`, `map_with_strategy`) is **never
called**.
The field is set to the target value verbatim.
Any Append/Prepend/DeepMerge metadata carried on the target's partial is inert
within the revert delta's application.

This means:

- The revert-delta builder does **not** need per-type helpers to force Replace
  semantics.
- The `ToPartial` conversion for the target value is fine as-is, regardless of
  which strategy the field normally uses.
- `schematic`'s `merge_nested_setting` (used for nested configs) has identical
  Option-gating, so nested fields work the same way.

**Caveat**: the stored target value may carry a strategy marker (e.g.
`PartialMergeableString::Merged { strategy: Append, ... }`) because it was
computed from a field whose default encoding uses that strategy.
This is harmless at revert-apply time (the strategy does not fire against a
`None` prev) and at subsequent-delta time (subsequent deltas use their own
`next.strategy` to drive merging; the stored `prev.strategy` is not consulted by
`string_with_strategy`, `vec_with_strategy`, or `map_with_strategy`).
The marker is inert metadata on a value that was produced by the revert
mechanism, not by the strategy it names.

### Conversation creation change

Currently, new conversations set `base_config` to the fully resolved `AppConfig`
(including environment variables, `-c` args, and CLI flags from the creation
invocation).
This bakes all override values into the base with no `ConfigDelta` stored and no
claims recorded.
A later `-C` has no claims to match against.

This RFD reshapes `base_config.json` to carry both the workspace snapshot and
the first invocation's per-directive deltas, preserving the `events.json`
cleanliness guarantee from [RFD 054] while giving first-invocation `-c`/`-C`
directives the same per-directive persistence as subsequent ones.

#### File format

`base_config.json` becomes a structured object with two fields:

```json
{
  "base": { /* PartialAppConfig — the workspace snapshot */ },
  "init": [
    { /* ConfigDelta — first creation-time directive */ },
    { /* ConfigDelta — next directive */ }
  ]
}
```

- **`base`** is the workspace `PartialAppConfig` — files merged via
  inheritance, without environment variables, `-c` args, or CLI flags.
  Same content as today's `base_config.json`; unchanged shape for hand-editing.
- **`init`** is an ordered list of `ConfigDelta` entries, one per directive
  (`-c` file, `-c key=value`, `-c '{...}'`, `-C ...`, or the trailing
  shortcut-flags batch) from the invocation that created the conversation.
  Each entry carries its diff, claims map, and any `unsets`.

Both parts are written once at conversation creation and immutable afterward.
Subsequent `-c`/`-C` deltas continue to land in `events.json` as under the
per-directive model described earlier — `events.json` stays free of leading
config blobs.

```txt
Before:  base_config.json = fully resolved PartialAppConfig  → no claims anywhere
After:   base_config.json = { base: workspace-only partial,
                              init: [creation-time ConfigDeltas] }
         events.json       = post-creation ConfigDeltas + ConversationEvents
```

Separating env vars from the workspace base is deliberate.
An env var like `JP_CFG_ASSISTANT_MODEL_PARAMETERS_REASONING=high` is user
intent for this session, not a workspace property.
Storing it as a creation- time delta in `init` (alongside `-c` and flag
contributions) keeps the boundary clean: `base` is workspace state, `init`
captures everything layered on at invocation time with full provenance.

This preserves [RFD 054]'s readability win: the workspace snapshot remains a
plain `PartialAppConfig` under `base`, inspectable and hand-editable;
`events.json` contains no creation-time config blob.
A creation-time `-c dev` (which may be hundreds of lines) lives inside `init`,
not in `events.json`.

#### API changes

- **`ConversationStream`**: the `base_config: Arc<AppConfig>` field is joined by
  an `init: Vec<ConfigDelta>` field holding the creation-time deltas in order.
  `config()`'s fold becomes `AppConfig::default()` → `base` partial → each
  `init` delta → each event-stream delta, using the `apply_config_delta` helper
  from Phase 1 for deltas.
- **`from_parts()` / `to_parts()`**: the `base_config` JSON component now has
  the `{ base, init }` shape.
  The signatures keep two JSON components on the outside (base\_config JSON
  value, events JSON vec) — the inner structure of `base_config.json` changes.
- **Per-event iterators** (`Iter`, `IterMut`, `IntoIter`) fold `base` and each
  `init` delta before walking events, so per-event config views stay consistent
  with `config()`.
- **`Workspace::create_and_lock_conversation`**: gain an `init_deltas:
  Vec<ConfigDelta>` parameter.
  Query-new passes the invocation's per-directive deltas in order.
- **Storage layer** (`jp_storage`): `Storage::persist_conversation` writes
  `base_config.json` as the `{ base, init }` object at creation.
  `load_conversation_stream` reads the new shape; see [Backward
  compat](#backward-compat-for-base_configjson) below.
- **Fork** (`crates/jp_cli/src/cmd/conversation/fork.rs`): clone the source's
  `base` and `init` together when seeding the new conversation.
  A source's initial overrides flow into the fork's `init` list unchanged.

#### Backward compat for `base_config.json`

The loader tries the new shape first and falls back to the legacy shape
transparently:

1. Parse `base_config.json` as JSON.
2. If the root is a JSON object with a `base` key → new format; use `base` as
   the workspace partial and `init` (defaulting to `[]` if absent) as the
   creation-time delta list.
3. Otherwise treat the root object as a legacy `PartialAppConfig` and wrap it as
   `{ base: <that>, init: [] }`.
   Legacy conversations therefore have an empty `init` list, matching their
   historical "no claims" behavior.
   `-C` is a no-op on their fields.

**Writers always emit the new shape, and explicitly migrate legacy files on
persist.** On every persist, `Storage::persist_conversation` inspects the
existing `base_config.json`:

- If the file is absent (new conversation), write the new shape.
- If the file is already in the new shape (`base` key present), copy it verbatim
  into the staging directory — preserves any user hand-edits, same as today's
  behavior.
- If the file is in the legacy flat shape, **rewrite** it as `{ base: <legacy
  content>, init: [] }` during the staging write.
  This is a one-time per-conversation migration; once rewritten, the file
  follows the "copy verbatim" path on subsequent persists.

This means legacy files are incrementally upgraded the next time the
conversation is touched.
Once all active conversations in a workspace have been persisted at least once
after this RFD lands, no legacy files remain and the legacy parser in
`load_conversation_stream` can be retired in a future release.

#### User-facing commands

`conversation edit` (`-b` / `--base-config`) and `conversation path`
(`--base-config`) (`crates/jp_cli/src/cmd/conversation/edit.rs`,
`crates/jp_cli/src/cmd/conversation/path.rs`) continue to point at
`base_config.json`.
Since the file now holds both the workspace snapshot and the creation-time
deltas in a single object, these flags surface the full initial state — no new
`--init-config` flag needed.

Users who hand-edit the file now see an object with `base` and `init` instead of
a flat partial.
The `base` subtree is the familiar `PartialAppConfig`; `init` is a JSON array of
deltas that can be edited if needed, though the expected workflow is `jp config
set` or a fresh `-c`/`-C` directive rather than direct edits.

### Parsing

`-C` reuses the same `KeyValueOrPath::from_str` parser as `-c`.
The clap definition wraps parsed values in `CfgDirective::Revert` and allows the
flag to appear with or without a value:

```rust
#[arg(
    short = 'C',
    long = "no-cfg",
    global = true,
    action = ArgAction::Append,
    value_name = "KEY=VALUE",
    value_parser = clap::builder::ValueParser::new(KeyValueOrPath::from_str)
        .map(CfgDirective::Revert),
)]
no_config: Vec<CfgDirective>,
```

Bare `--no-cfg` (no value) has no defined meaning in this RFD and is rejected at
parse time — `-C` / `--no-cfg` always requires a value.
A later RFD may define a meaning for the bare form.

Positive (`-c`) and negative (`-C`) args are merged into a single
`Vec<CfgDirective>` preserving command-line order.
Since clap does not preserve cross-field ordering between two separate `Vec`
args, this requires a manual `clap::FromArgMatches` implementation that recovers
positions via `ArgMatches::indices_of(..)` and sorts the merged list by index.
`ToolDirectives` in `crates/jp_cli/src/cmd/query.rs` implements the same pattern
for `--tool`/`--no-tools` (see [RFD 008]); the `-c`/`-C` merge reuses the same
approach.

Users who want to undo post-creation changes to a conversation — i.e., revert
to the `base + init` state captured at creation time — can use targeted `-C
<source>` for specific sources.
A dedicated keyword that expands to `base + init` is out of scope; the `init`
list introduced here preserves the infrastructure needed to add such a keyword
later.

### Examples

#### Basic cross-invocation revert

```sh
jp query -n -c dev            # invocation 1
jp query -C dev -c committer  # invocation 2
```

**Invocation 1**: `base_config.json` is written with `base` = workspace files
and `init` = one `ConfigDelta` per directive (dev's contribution plus any
env-var overrides).
Claims (`assistant.name → hash(dev)`, `conversation.tools.read_file.enable →
hash(dev)`, etc.) land on dev's entry in `init`.

**Invocation 2**: `-C dev` resolves dev, computes hash, walks claims.
Dev owns `assistant.name` → revert.
Dev owns `tools` → revert.
Then `-c committer` layers on top.
Final delta captures both the revert and committer's additions.

#### Overlapping sources — same field, same value

```sh
jp query -n -c dev      # invocation 1: tools=[read_file]
jp query -c architect   # invocation 2: tools=[read_file]
jp query -C dev         # invocation 3: revert dev
```

**Invocation 1**: delta claims `conversation.tools.read_file.enable →
hash(dev)`.

**Invocation 2**: architect also sets `tools.read_file.enable = true`.
The config value doesn't change, so the field is NOT in the delta's diff.
But architect's partial DOES set it, so the claims map records
`conversation.tools.read_file.enable → hash(architect)`, overwriting dev's
claim.

**Invocation 3**: `-C dev` checks: who owns
`conversation.tools.read_file.enable`?
Most recent claim is `hash(architect)` (from invocation 2).
Dev is not the owner.
**Skip.** Tools remain enabled.

#### Shortcut flag override

```sh
jp query -c dev           # invocation 1: model set by dev
jp query --model gpt-4o   # invocation 2: model overridden
jp query -C dev           # invocation 3: revert dev
```

**Invocation 1**: delta claims `assistant.model.id → hash(dev)`.

**Invocation 2**: `--model gpt-4o` is a shortcut flag.
Delta diff has the new model value.
Claims map records `assistant.model.id → hash("kv:assistant.model.id=gpt-4o")`,
overwriting dev's claim.

**Invocation 3**: `-C dev` checks: who owns `assistant.model.id`?
Most recent claim is the kv identity from invocation 2 — not in dev's target
set.
**Skip.** The explicit `--model` override is preserved.

#### Key-value revert (value-based)

```sh
jp query -c assistant.name=DevBot   # invocation 1
jp query -C assistant.name=DevBot   # invocation 2
```

**Invocation 1**: kv records a delta with `claims["assistant.name"] =
vec![hash("kv:assistant.name=DevBot")]`.
The resolved value of `assistant.name` is `DevBot`.

**Invocation 2**: `-C assistant.name=DevBot` reads the current resolved value
(`DevBot`), compares to the target (`DevBot`).
Match.
Walk back through `ConfigDelta` events; for each delta, check whether
`assistant.name`'s resolved value at that point was still `DevBot`.
Stop at the first earlier state where it was different (or at base).
Emit a revert delta setting `assistant.name` to that earlier value.

If invocation 2 were `-C assistant.name=Different`, the current value (`DevBot`)
does not equal the target (`Different`).
**Skip** with diagnostic — the user asked to undo a value that isn't currently
set.

#### Key-value undo of a file-set value

```sh
jp query -c dev                      # dev.toml sets assistant.name=DevBot
jp query -C assistant.name=DevBot    # undo the DevBot assignment
```

**Invocation 1**: delta claims `assistant.name → hash(dev)` (dev's file
identity, not kv identity).
Current value: `DevBot`.

**Invocation 2**: `-C assistant.name=DevBot` is value-based.
Current value = `DevBot`, target = `DevBot`.
Match.
Walk back through deltas, skipping each one where `assistant.name` was still
`DevBot`, and stop at the first earlier state where it differed.
For this scenario, the walk reaches base (dev's delta was the only claim on
`assistant.name`), so the revert emits the base value.

This works because kv `-C` doesn't care about source identity — only about the
field's current value.
The `dev` delta is still responsible for the claim on other fields, but
`assistant.name` is now back to base.

#### Within-invocation layering then revert

```sh
jp query -n -c dev -c architect    # invocation 1: layer both
jp query -C architect              # invocation 2: drop architect
```

**Invocation 1**: with per-directive deltas, two deltas are persisted:

- Delta A: dev's fields, claims under `hash(dev)`.
- Delta B: architect's fields (including any shared with dev), claims under
  `hash(architect)` (overwriting dev's claim in the current claims state for
  shared fields).

**Invocation 2**: `-C architect` target = `{hash(architect)}`. Scope = fields
currently claimed by architect.
Walk back for each scope field:

- Shared field (both dev and architect set it): delta B's claim is architect →
  walk past.
  Delta A's claim is dev → stop.
  Revert value = delta A's value (dev's value).
- Architect-only field: delta B's claim is architect → walk past.
  No prior delta claims it → reach base.
  Revert value = base config value.

Net effect: architect's contributions are removed, dev's values re-emerge for
shared fields, dev-only fields are untouched.
The user's "drop architect, keep dev" intent holds.

#### Repeated claimant across invocations (A → B → A)

```sh
jp query -n -c dev        # invocation 1: dev sets assistant.name
jp query -c architect     # invocation 2: architect overwrites
jp query -c dev           # invocation 3: dev overwrites again
jp query -C dev           # invocation 4: revert dev
```

**Invocation 1**: delta \#1 claims `assistant.name → hash(dev)`.

**Invocation 2**: delta \#2 claims `assistant.name → hash(architect)`.

**Invocation 3**: delta \#3 claims `assistant.name → hash(dev)` (overwriting
architect's claim).

**Invocation 4**: `-C dev`.
Walk back:

1. Delta \#3: claim on `assistant.name` is `hash(dev)` — in the target set.
   Mark for revert.
2. Walk past claims whose hash is in dev's target set.
   Delta \#2: claim is `hash(architect)` — not in target set.
   Stop.
3. Revert value = `assistant.name` as it stood after delta \#2 (architect's
   value).

Architect's value re-emerges, not the base-config value.
This preserves the intuition "undo dev, leave everything else alone" —
architect's earlier claim is still valid.

Under per-directive delta persistence, the A→B→A pattern works identically
whether invocations 1–3 were separate or collapsed into within-invocation
sequences like `jp query -c dev -c architect -c dev`.
Each directive persists its own delta, so intermediate claims aren't squashed.

#### Source file edited between invocations

```sh
jp query -n -c dev           # invocation 1: dev.toml sets assistant.name and model
# <user edits dev.toml, removes the assistant.name setting>
jp query -C dev              # invocation 2: revert dev
```

**Invocation 1**: delta claims `assistant.name → hash(dev)` and
`assistant.model.id → hash(dev)`.

**Invocation 2**: `-C dev` computes `hash(dev)` (still the same, since either
`id` or path-hash is edit-stable for content edits).
Walk the current claims state: both `assistant.name` and `assistant.model.id`
are still claimed by dev.
Scope = {`assistant.name`, `assistant.model.id`}.
Revert both.

Critically, `dev.toml`'s current contents are irrelevant to scope computation.
Even though the file no longer sets `assistant.name`, the original claim is
honored.
This is the claim-history guarantee — the fundamental reason scope comes from
claim history, not from loading the file and re-deriving its fields.

## Drawbacks

**Claims add storage overhead.** Each `ConfigDelta` gains a `BTreeMap` of field
paths to source hashes.
For a typical persona file setting 10-20 fields, this is a few hundred bytes per
delta.
Negligible in practice.

**Per-directive delta granularity inflates event count.** Persisting one
`ConfigDelta` per `-c`/`-C` directive (instead of per invocation) means a
typical invocation with 2–3 config args plus shortcut flags now writes 3–4
events per invocation rather than 1.
The cumulative effect on `events.json` is small in bytes (each delta is a diff
plus claims, not a full config snapshot), but `events_count`, which counts all
events in the file (see RFD 054 §Non-Goals), becomes more inflated.
UIs that surface "N events" will see higher numbers for conversations that made
heavy use of layered config.
A future turn-based counting mechanism (deferred) would fix this.

**Conversation creation change.** `base_config.json` changes shape: it becomes a
JSON object `{ base: PartialAppConfig, init: Vec<ConfigDelta> }` instead of a
flat `PartialAppConfig`.
Code that reads `base_config.json` expecting the flat shape needs to handle the
new form (the loader falls back transparently on legacy files).
The steady-state persist cost is unchanged: `base_config.json` is written once
at creation, subsequent persists write only `events.json` and `metadata.json`.

**Implementation cost.** The claims system touches many parts of the config
pipeline:

- **Delta plumbing rewrite** in `jp_conversation`: the hand-rolled
  `deserialize_config_delta` and `add_config_delta` helpers need explicit
  updates; a new `apply_config_delta` helper replaces six direct
  `partial.merge(..)` call sites.
- **`apply_cli_config` signature change**: the `IntoPartialAppConfig` trait and
  each `apply_*` helper gain a `claims: &mut BTreeMap<...>` parameter to record
  per-flag identities.
- **`ClaimIdentity` impls** for the 7 set-like and identity-bearing vec fields
  (~6 unique element types; see [Claim granularity](#claim-granularity) for the
  full classification).
  Duplicate-capable vec fields do not need `ClaimIdentity` — they use
  whole-field claims.
  Most impls are one-liners (strings, paths), but each requires a deliberate
  identity choice.
- **Conversation API changes**: `ConversationStream` gains an `init:
  Vec<ConfigDelta>` field; `from_parts()` / `to_parts()`, the three iterator
  types, `Workspace::create_and_lock_conversation`, storage layer, and fork path
  are updated to carry it through.
  `base_config.json`'s file shape changes to `{ base, init }` with a
  legacy-shape fallback in the loader.
- **CLI surface**: manual `clap::FromArgMatches` to merge `-c` and `-C` with
  preserved ordering, mirroring `ToolDirectives`.

Each piece is small but the surface area is broad.
Phase 1 alone adds fields, a trait, a helper, and ~6 per-type impls.

## Alternatives

### Snapshot stack (no provenance)

Replace the single-accumulator merge with a stack of intermediate snapshots.
`-C` walks the stack backwards comparing values to find the revert target.
Simpler to implement (no claims, no serialization changes), but fundamentally
limited: when two sources set the same field to the same value, value comparison
cannot distinguish them.
The tools example (both `dev` and `architect` enabling `read_file`) would
incorrectly disable the tool when reverting dev.
Rejected because the common case of overlapping tool configurations makes this a
real problem, not a theoretical edge case.

### Per-flag ConfigDelta storage

Store one `ConfigDelta` per `-c` flag instead of one per invocation.
Provides finer-grained history but does not solve the core problem: if two `-c`
flags in the same invocation set the same field to the same value, the delta
diffs are identical.
Only provenance tracking distinguishes them.
Rejected as insufficient on its own, though it could complement claims for
within- invocation revert precision.

### Full provenance tracking (tagged values)

Replace `Option<T>` fields in `PartialAppConfig` with `Tagged<T>` that carries a
source identifier.
The most architecturally complete solution, but requires changing the
representation of every config field.
Over-engineered for the immediate use case.
The claims map achieves the same result for revert purposes without touching the
config type system.

## Non-Goals

- **Direct stream editing.** `-C` does not remove or modify existing
  `ConfigDelta` events in the conversation stream.
  It influences the *next* delta by changing what the pipeline produces.
  The stream remains append-only.
- **Provenance display.** Showing which source contributed which field is useful
  but orthogonal.
  See [RFD 060].

## Risks and Open Questions

### File-based and key-value `-C` have different scoping rules

File-based `-C <source>` scopes by **source identity**: fields whose current
claim is in the target's identity set.
Key-value `-C foo=value` scopes by **current value**: revert `foo` if its
resolved value equals `value`, walking back past all claims on the field
regardless of owner.

The asymmetry is intentional:

- `-C <source>` answers "undo this source's influence."
  Identity matching is the natural scope.
- `-C foo=value` answers "undo this specific assignment."
  Value matching is the natural scope.
  Users typing an explicit value are asserting both the field and the expected
  current state; if the value doesn't match, the command skips with a
  diagnostic.

Consequences worth noting:

- `-C foo=A` after `-c foo=A -c foo=B` does nothing (current value is `B`, not
  `A`).
  The user should use `-C foo=B` to undo the current value, or use `-C <source>`
  to revert by source identity.
- `-C foo=EnvVal` when `JP_CFG_FOO=EnvVal` is set **does** revert the env-set
  field, because the user typed the value explicitly.
  This bypasses the env-unclaim protection that applies to file-based `-C`.
  File-based `-C` still respects env unclaims.
- `-C assistant.name=DevBot` after `-c dev` (where dev set
  `assistant.name=DevBot`) **does** revert — kv `-C` is value-based, so the
  originating source doesn't matter.

File-based and kv `-C` compose predictably within one invocation.
A mix like `-C dev -C assistant.name=DevBot` runs each independently against the
current claims state at its position in the left-to-right directive loop.

### Claim granularity

Claims are recorded at **leaf level** — the finest granularity available.
When dev sets `tools.read_file.enable = true` and architect sets
`tools.read_file.run = "ask"`, the claims are:

```txt
conversation.tools.read_file.enable → hash(dev)
conversation.tools.read_file.run    → hash(architect)
```

`-C dev` reverts `enable` only, leaving architect's `run` setting untouched.
This is more precise than entry-level claims (`conversation.tools.read_file`),
which would revert the entire tool config including fields dev never set.

Map-level claims (`conversation.tools → hash(dev)`) would be broken entirely:
architect enabling `write_file` would overwrite dev's claim on the whole map,
and `-C dev` would skip reverting `read_file` because architect "owns" tools.

Leaf-level paths are computed by a **typed walker** over `PartialAppConfig` and
its nested partial types — not a generic serde-JSON walk — so that vec
elements can be dispatched to their type's `ClaimIdentity` implementation (see
below).
Each partial type contributes a `field_paths_into(&self, prefix: &str, out: &mut
Vec<String>)` method that collects the leaf paths for its own fields.
This mirrors the existing `AssignKeyValue` dispatch shape — a per-type walk
that knows how to treat each field's type.

#### Vec claim granularity: set-like, identity-bearing, duplicate-capable

Vec fields cannot use positional indices for claims — if a later delta removes
an element, all subsequent indices shift and a revert would corrupt the array.
The RFD classifies each vec field into one of three categories with different
claim granularity:

| Category              | Claim granularity                                                                                   | Revert mechanic                                            |
| --------------------- | --------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| **Set-like**          | Per-element via `ClaimIdentity`                                                                     | `remove_element(path, identity)` filters matching elements |
| **Identity-bearing**  | Per-element via `ClaimIdentity` with `json_identity` fallback when the natural identifier is absent | Same as set-like                                           |
| **Duplicate-capable** | Whole-field claim (path only, no element suffix)                                                    | Revert emits the entire prior value of the field           |

The distinction matters because `Vec<String>` fields like
`ToolCommandConfig.args` permit repeated identical entries (e.g.
`args = ["-I", "path1", "-I", "path2"]`).
A per-element identity of `self.clone()` would make both `-I` entries
indistinguishable, and `remove_element(path, "-I")` would wipe both when the
user meant to revert only one source's contribution.
Whole-field claims dodge the ambiguity: dev owns `command.args` as a unit,
architect overwrites that claim as a unit, and `-C architect` restores dev's
complete value for the field.

##### `ClaimIdentity` trait

For set-like and identity-bearing fields, each element type implements:

```rust
/// Produce a stable identity string for an element in a claimed vec field.
///
/// The identity is used as the key in claims for array positions (e.g.
/// `conversation.attachments[<identity>]`). It must be deterministic
/// across runs, stable across the lifetime of the element, and
/// **total** — every distinguishable element value produces a
/// distinguishable identity.
pub trait ClaimIdentity {
    fn claim_identity(&self) -> String;
}
```

A `json_identity` helper is available as explicit opt-in for types with no
natural identifier:

```rust
pub fn json_identity<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}
```

##### Per-field classification

The config has 15 Vec-typed fields today:

| Field path                                              | Element type            | Category          | Identity strategy                                                 |
| ------------------------------------------------------- | ----------------------- | ----------------- | ----------------------------------------------------------------- |
| `config_load_paths`                                     | `RelativePathBuf`       | Set-like          | `as_str()`                                                        |
| `extends`                                               | `ExtendingRelativePath` | Set-like          | `AsRef::<RelativePath>::as_ref().as_str()`                        |
| `conversation.attachments`                              | `AttachmentConfig`      | Set-like          | `to_url().map(\|u\| u.to_string()).unwrap_or_else(json_identity)` |
| `providers.llm.anthropic.beta_headers`                  | `String`                | Set-like          | identity (self) — deduped by `vec_dedup`                          |
| `assistant.system_prompt_sections`                      | `SectionConfig`         | Identity-bearing  | `tag.clone().or(title.clone()).unwrap_or_else(json_identity)`     |
| `conversation.inquiry.assistant.system_prompt_sections` | `SectionConfig`         | Identity-bearing  | same as above                                                     |
| `assistant.instructions`                                | `InstructionsConfig`    | Identity-bearing  | `title.clone().unwrap_or_else(json_identity)`                     |
| `assistant.instructions.*.items`                        | `String`                | Duplicate-capable | whole-field claim                                                 |
| `assistant.instructions.*.examples`                     | `ExampleConfig`         | Duplicate-capable | whole-field claim                                                 |
| `assistant.model.parameters.stop_words`                 | `String`                | Duplicate-capable | whole-field claim                                                 |
| `conversation.tools.*.command.args`                     | `String`                | Duplicate-capable | whole-field claim                                                 |
| `conversation.tools.*.enumeration`                      | `serde_json::Value`     | Duplicate-capable | whole-field claim                                                 |
| `editor.envs`                                           | `String`                | Duplicate-capable | whole-field claim                                                 |
| `providers.mcp.arguments`                               | `String`                | Duplicate-capable | whole-field claim                                                 |
| `providers.mcp.variables`                               | `String`                | Duplicate-capable | whole-field claim                                                 |

Additionally, `IndexMap<String, T>` fields use their keys as natural identities
— no `ClaimIdentity` impl needed because the string key IS the identity.
The typed walker descends into them to record per-entry claims:

- `conversation.attachment.params: IndexMap<String, Value>`
- `conversation.tools.*.parameters: IndexMap<String, ToolParameterConfig>`
- `conversation.tools.*.questions: IndexMap<String, QuestionConfig>`
- `conversation.tools.*.options: IndexMap<String, JsonValue>`
- `plugins.command: IndexMap<String, CommandPluginConfig>`
- `providers.mcp: IndexMap<String, McpProviderConfig>`
- `providers.llm.aliases: IndexMap<String, ModelIdConfig>`
- `template.values: IndexMap<String, JsonValue>`
- `model.parameters.other: IndexMap<String, JsonValue>`
- `ToolParameterConfig.properties: IndexMap<String, ToolParameterConfig>`

##### Representative implementations

Set-like and identity-bearing types implement `ClaimIdentity` explicitly.
Field accessors match the actual types in `crates/jp_config/src`:

```rust
impl ClaimIdentity for AttachmentConfig {
    fn claim_identity(&self) -> String {
        // `to_url` produces a canonical URL form; fallback to a
        // deterministic JSON hash on the unlikely construction error.
        self.to_url()
            .map(|u| u.to_string())
            .unwrap_or_else(|_| json_identity(self))
    }
}

impl ClaimIdentity for ExtendingRelativePath {
    fn claim_identity(&self) -> String {
        // Path component is the identity; strategy wrapper is ignored
        // because two entries differing only in strategy refer to the
        // same file.
        AsRef::<relative_path::RelativePath>::as_ref(self)
            .as_str()
            .to_string()
    }
}

impl ClaimIdentity for SectionConfig {
    fn claim_identity(&self) -> String {
        // Prefer explicit human-visible identifiers; fall back to a
        // JSON hash for sections that carry neither.
        self.tag
            .clone()
            .or_else(|| self.title.clone())
            .unwrap_or_else(|| json_identity(self))
    }
}

impl ClaimIdentity for InstructionsConfig {
    fn claim_identity(&self) -> String {
        // `title` is `Option<String>`; fall back to json_identity so
        // the strategy is total.
        self.title
            .clone()
            .unwrap_or_else(|| json_identity(self))
    }
}

impl ClaimIdentity for String {
    fn claim_identity(&self) -> String { self.clone() }
}
```

Resulting claim paths for set-like and identity-bearing fields:

```txt
conversation.attachments[foo.rs]            → hash(dev)
conversation.attachments[/abs/path/bar.rs]  → hash(dev)
config_load_paths[.jp/agents]               → hash(architect)
assistant.instructions[Rust]                → hash(dev)
```

Duplicate-capable fields use a path-only claim:

```txt
conversation.tools.fs_read.command.args     → hash(dev)
providers.mcp.arguments                     → hash(workspace)
editor.envs                                 → hash(user-global)
```

##### Revert mechanics by category

- **Set-like and identity-bearing**: revert puts each element to be removed into
  the revert delta's `unsets`, keyed by `path[identity]`.
  The stream fold applies these by calling `remove_element(path, identity)` on
  the typed vec, which filters out elements whose `claim_identity()` equals the
  stored identity.
  Before marking an element for removal, the revert algorithm checks whether the
  element existed in the config state *before* the source first claimed it (same
  walk-back as scalar fields).
  If it did, the source redundantly declared it and reverting does not remove
  it.
- **Duplicate-capable**: revert treats the field as an atomic value.
  The walk-back locates the field's state at the prior non-target claim (or
  base) and the revert delta carries the entire vec value for the field.
  If the field must be reset to empty/missing, the path goes into `unsets`
  (without an element suffix), and `stream.config()` applies `unset(path)`
  during replay.

The asymmetry means duplicate-capable fields lose element-level precision: `-C
architect` when both dev and architect contributed restores the whole field to
dev's state, discarding architect's additions entirely.
That is the honest semantics — element-level revert on a duplicate-tolerant
list requires occurrence counting, which is out of scope for this RFD.

An explicit opt-in keeps the classification visible in code.
A future `#[derive(ClaimIdentity)]` macro or a `#[claim_granularity =
"whole_field"]` attribute can be added later if the set of duplicate-capable
fields grows significantly.

### Conversation-ID inheritance

Conversation-ID inheritance (`--cfg jp-c<id>`, defined in a future RFD) expands
another conversation's resolved config into a fully-populated partial.
Like any `Apply` source, an inheriting partial generates claims under its source
identity (`hash(conversation_id)`) and persists as a `ConfigDelta` event.
The claims pipeline is uniform — no special-case logic for inheritance.

**Inheritance collapses claim granularity.** A single source identity
(`hash(conversation_id)`) claims every field in the inherited partial.
The inner provenance from the source conversation — which `dev` or `architect`
originally claimed each field — is lost in the target conversation.
This is acceptable: inheritance is a wholesale "adopt this state" operation, and
`-C jp-c<id>` undoes it wholesale.
Users who need finer-grained control over inherited influence should layer
sources explicitly (`-c jp-c<id> -c overrides.toml`) rather than relying on
preserved provenance from the source.

### Extends sub-files

Config files can use `extends` to pull in other files.
When `-c dev` resolves to `dev.toml`, its `extends` directives are folded into a
single `PartialAppConfig` before the file is returned from
`load_partial_at_path`.
By the time the claims pipeline sees the partial, all extends content is already
merged in.

The natural consequence: **fields contributed by extended files are credited to
the parent file's claim identity**, not to each extended file individually.
If `dev.toml` extends `config.d/tools.toml`, a field set in `tools.toml` is
claimed by dev's hash.
`-C dev` reverts it along with the rest of dev's influence.

This is the correct behavior: the user typed `-c dev`, not `-c dev.d/tools`.
Finer-grained provenance for extends sub-files is orthogonal (see [RFD 060]) and
belongs in the provenance-display layer, not the claims layer.

### Claims map size for large config files

A config file that sets many fields produces a large claims map.
In practice, persona files set 10-30 fields.
If a config file sets hundreds of fields (e.g. a full config dump), the claims
map grows accordingly.
This is bounded by the total number of config fields in `AppConfig` and unlikely
to be a performance concern.

### Renamed or removed config fields

If a config field is renamed or removed across JP versions, old claims keyed on
the previous path become orphaned.
These are harmlessly ignored — `-C` queries against current field paths and
skips entries it does not recognize.

### Missing claims: UX and diagnostics

`-C <target>` is a silent no-op in several cases.
`-C` emits an internal diagnostic (not an error; the command still succeeds):

- **Legacy conversation, file-based `-C <source>`**: no `ConfigDelta` has
  `claims` populated, so `claims_state` is empty.
  File-based `-C` is a no-op — there's no provenance to scope against.
  Key-value and JSON-object `-C` forms are value-based and can still work on
  legacy conversations when a post-creation delta mutated the target field.
- **Unapplied source**: `-C dev` where `dev` was never applied to this
  conversation.
  Scope is empty.
- **Mismatched kv value**: `-C foo=A` where the field's current resolved value
  is not `A` (a later assignment set it to something else, or nothing set it at
  all).
- **Deleted non-workspace source**: `-C dev` where a user-local or
  user-workspace `dev.toml` is missing.
  These bases use scan-based resolution, so a missing file produces no candidate
  hash.
  Workspace files remain targetable after deletion because the claim stores a
  path-derived identity alongside any `id` hash.

Diagnostic examples:

```
No fields currently claimed by 'dev' in this conversation.
assistant.name is currently 'Other', not 'DevBot'.
Cannot resolve 'dev' for revert: dev.toml is missing and its identity requires reading the file.
```

Users can inspect current claims via `jp conversation show --claims` (future
work) or read `events.json` directly.

### Backward compatibility

Old conversation streams have `ConfigDelta` events without claims.
The `#[serde(default)]` attribute initializes an empty map for these.
For fields without claims, `-C` skips rather than guessing — silently falling
back to value-diff guessing would produce unpredictable results.
Legacy conversations therefore keep working as before; `-C` is simply a no-op on
their fields.
Precise revert is available on any new conversation created after this feature
lands.

Old `base_config.json` files already contain the fully-resolved config (env +
`-c` + CLI flags) from their creation invocation.
On first persist after this RFD lands, the file is migrated in place to the new
`{ base: <legacy content>, init: [] }` shape (see [Phase
3](#phase-3-conversation-creation-change)).
The migration is content-preserving; the fully-resolved config moves under
`base` with an empty `init` list, and `-C` remains a no-op on these fields (no
claims to match).
Precise revert is only available for conversations created after this feature
lands — the legacy file's embedded overrides cannot be separated from workspace
config without provenance data that was never recorded.

## Implementation Plan

### Phase 1: Delta plumbing, `ClaimIdentity`, and typed walker

This phase lays the data-model foundation without wiring `-C` itself.

**`ConfigDelta` changes** in `crates/jp_conversation/src/stream.rs`:

- Add `claims: BTreeMap<String, Vec<String>>` and `unsets: Vec<String>` fields
  to `ConfigDelta`.
  The claims-map value is a list: empty means explicit unclaim, non-empty means
  the field is claimed by any of the listed identities.
- Extend `deserialize_config_delta` to read `claims` and `unsets` from the raw
  `Value`.
  Struct-level `#[serde(default)]` is not enough because this is a hand-rolled
  deserializer.
- Rework `add_config_delta` to preserve `claims` and `unsets` through the
  diff-recomputation step, and to emit a delta when any of them (not just
  `delta`) is non-empty.
- Introduce `apply_config_delta(partial: &mut PartialAppConfig, delta:
  &ConfigDelta)` that both merges `delta.delta` and applies `delta.unsets`.
  Replace all six direct `partial.merge(..)` call sites (`config()`,
  `IntoIter::next`/`next_back`, `Iter::next`/`next_back`, `IterMut::next`) with
  this helper.

**Config-layer changes** in `jp_config`:

- Add the `id` field to `AppConfig` as an optional field.
  The `schematic` `#[derive(Config)]` macro auto-generates the corresponding
  `Option<String>` on `PartialAppConfig`.
  Strip it during config resolution — it feeds source identity, not merged
  state.
- Implement source hash computation (hash `id` if present, resolved or would-be
  path otherwise; would-be path enables `-C` against deleted path-hashed files).
- Add the `ClaimIdentity` trait with implementations for the 7 set-like and
  identity-bearing vec fields listed in [Claim granularity](#claim-granularity)
  — approximately 6 unique element types (`RelativePathBuf`,
  `ExtendingRelativePath`, `AttachmentConfig`, `SectionConfig`,
  `InstructionsConfig`, `String`).
  Most impls are trivial (string identity, path accessor); a few opt into
  `json_identity` as a fallback.
  Duplicate-capable vec fields do not get `ClaimIdentity` — they use
  whole-field claims instead.
- Add the typed `field_paths_into(&self, prefix, out)` walker on each partial
  type, mirroring the `AssignKeyValue` dispatch shape.
  The walker descends into both `Vec<T: ClaimIdentity>` and `IndexMap<String,
  T>` fields.
- Add `unset(path)` and `remove_element(path, identity)` methods on
  `PartialAppConfig` and nested partial types.

Ensure backward-compatible deserialization: legacy deltas without `claims` or
`unsets` load with empty defaults.

Tests:

- Claims and unsets serialization round-trip via `deserialize_config_delta`
  (stable key ordering via `BTreeMap`).
- Backward compatibility with old `ConfigDelta` events (no claims field).
- `unset` and `remove_element` for representative field types (scalar optional,
  nested struct, vec element removal).
- `claim_identity()` stability for each of the 6 vec element types implementing
  the trait.
- `apply_config_delta` produces identical results in `config()` and the three
  iterator types when `unsets` is in play.

Can be merged independently — claims fields exist but nothing populates them
yet.

### Phase 2: Claims recording and per-directive delta emission

Update `apply_cfg_args` and `apply_cli_config` to build claims maps and emit
per-directive deltas.

**Claim recording rules:**

- `-c` file args record claims keyed on the file's source identity (`id` or
  resolved-path hash).
  File partials run through `field_paths_into` to enumerate which leaves the
  file sets.
- `-c` key-value args record claims keyed on the kv identity `hash("kv:" +
  field_path + "=" + canonical_value)` (still used for apply-side provenance
  even though kv `-C` is value-based).
- `-c` JSON-object args (e.g.
  `-c '{"assistant":{"name":"x"}}'`) are pre-expanded via `try_merge_object`
  into per-leaf kv assignments, each recording its own kv identity.
- Shortcut flags go through their `apply_*` helper, which records the
  `(field_path, canonical_value)` claim alongside the partial mutation.
  Each helper gains a `claims: &mut BTreeMap<String, Vec<String>>` parameter.
- Environment variable overrides (`JP_CFG_*`) record `claims[path] = vec![]`.
  In-invocation env unclaims seed the starting claims state for phase 2 (see
  [Current claims state](#current-claims-state)).

**Per-directive delta emission:**

- `apply_cfg_args` tracks the partial state before each `-c`/`-C` directive and
  emits a separate `ConfigDelta` after applying it.
  Each delta carries that directive's incremental partial diff, its claims
  contribution, and any `unsets` for revert directives.
- Shortcut flags batch into a single trailing `ConfigDelta` emitted after all
  `-c`/`-C` directives.
  The claims map covers all fields that any flag touched.
- `get_config_delta_from_cli` (today a one-shot "compute the final diff") is
  replaced by per-directive emission inside the apply loop.
  Each emitted delta is persisted via `add_config_delta`.

**Value canonicalization:** parse to the field's typed value, then serialize
back via the field's canonical serde form.
Model aliases are resolved before hashing — `--model opus` and `-c
assistant.model.id=anthropic/claude-opus-4-6` must produce the same identity.

Tests:

- Claims are recorded correctly for each source type (file, kv, JSON-object,
  shortcut flag, env).
- Per-directive deltas: `-c dev -c architect` produces two `ConfigDelta` events,
  not one.
- Within-invocation A→B→A persists three deltas with the correct intermediate
  claims.
- Shortcut flags batch into a single trailing delta.
- kv and shortcut flag produce matching identities when they target the same
  field with the same resolved value.
- JSON-object `-c` expands to per-leaf kv claims identical to the equivalent
  repeated `-c key=value` invocation.
- Env vars produce `vec![]` entries in the starting claims state.
- `-C` walks back through per-directive deltas correctly for the `-c dev -c
  architect → -C architect` case.

Depends on Phase 1.
Can be merged independently.

### Phase 3: Conversation creation and `base_config.json` shape change

This phase reshapes `base_config.json` to carry creation-time per-directive
deltas alongside the workspace snapshot.

**`ConversationStream`** (`crates/jp_conversation/src/stream.rs`):

- Add `init: Vec<ConfigDelta>` as a first-class field, between `base_config` and
  `events`.
- Update `config()` to fold `AppConfig::default()` → `base` partial → each
  `init` delta → each event delta, using the `apply_config_delta` helper from
  Phase 1 for deltas.
- Update `Iter`, `IterMut`, and `IntoIter` to fold `base` and each `init` delta
  before walking events, so per-event views match `config()`.
- `from_parts()` / `to_parts()` keep their two-component signatures
  (base\_config JSON value, events JSON vec); the JSON shape of the base\_config
  component changes to `{ base, init }`.

**Storage layer** (`jp_storage`):

- `Storage::persist_conversation` (`crates/jp_storage/src/lib.rs`, reached via
  the `PersistBackend::write` trait method on the concrete backend) writes
  `base_config.json` in the new `{ base: PartialAppConfig, init: [ConfigDelta,
  …] }` shape.
  The existing copy-if-exists path gets a third branch:
  - File absent → write new shape from the in-memory value (new conversations).
  - File present in new shape → copy verbatim (preserves user hand-edits;
    matches today's behavior).
  - File present in legacy flat shape → **rewrite in place** during the staging
    write, wrapping the legacy content as `{ base: <legacy>, init: [] }`.
    One-time per-conversation migration.
    Detection is a cheap JSON shape check (is the root an object with a `base`
    key?).
    The legacy parse only runs when the shape check fails.
- `load_conversation_stream` also handles the legacy shape transparently (wrap
  on read), so a conversation loaded between the migration landing and its next
  persist still behaves correctly.
- `MANAGED_FILES` is unchanged — `base_config.json` is still managed as today;
  only its contents change.
- The legacy parser and rewrite logic can be retired in a future release once
  all active conversations have been persisted at least once since the migration
  landed.

**Workspace API** (`jp_workspace`):

- `Workspace::create_and_lock_conversation` gains an `init: Vec<ConfigDelta>`
  parameter.
- `create_and_lock_conversation_with_id` same change.
- Downstream callers (query-new in `query.rs`, `fork_conversation`) pass the
  appropriate list.

**Fork path** (`crates/jp_cli/src/cmd/conversation/fork.rs`):

- `fork_conversation` forwards both the source's `base` partial and its `init`
  delta list to the new conversation.
  Event-replay is unchanged — only the initial-state seeding changes to carry
  `init`.

**Query-new path** (`crates/jp_cli/src/cmd/query.rs`):

- New conversations pass the invocation's per-directive `ConfigDelta` list (one
  entry per `-c`/`-C` directive, plus the trailing shortcut-flag delta if any)
  as `init` to `create_and_lock_conversation`.
  The `base` partial is the pure workspace config.

**User-facing commands**: no new flags.
`conversation edit -b` and `conversation path --base-config` continue to point
at `base_config.json`, which now holds the full initial state in one file.

Existing conversations are migrated in place: their `base_config.json` is
rewritten to `{ base: <legacy content>, init: [] }` on the next persist after
this RFD lands (see the Storage layer bullets above).
`-C` is a no-op on their fields because the legacy content has no claims to
match — the migration is content-preserving but does not recover provenance
that was never recorded.
Precise revert only works on conversations created after this feature lands.

Tests:

- New conversations produce `base_config.json` in the new shape.
- `from_parts()` / `to_parts()` round-trip a stream with a non-empty `init`
  list.
- `config()` produces the correct resolved result for the base + init + events
  fold.
- Legacy flat `base_config.json` loads as `{ base, init: [] }` and the stream
  behaves as today.
- Legacy file is rewritten to the new shape on first persist: create a
  conversation directory with a flat `PartialAppConfig` `base_config.json`,
  persist the loaded stream unchanged, assert the on-disk file now has the `{
  base, init }` shape with the legacy content verbatim under `base`.
- Once rewritten, subsequent persists copy the new-shape file verbatim (the
  user-hand-edit preservation path still works).
- Fork propagates `init` from source: a forked conversation carries the same
  creation-time deltas.
- `conversation path --base-config` still returns the `base_config.json` path.

Depends on Phase 2.
Can be merged independently.

### Phase 4: `-C` flag and claim-history-driven revert

Add `CfgDirective` wrapper enum with `Apply` and `Revert` variants wrapping
`KeyValueOrPath`.

**CLI wiring**:

- Wire `-C` / `--no-cfg` in clap as the negative counterpart of `-c` / `--cfg`.
  The flag requires a value (file path, `key=value`, or JSON object); bare
  `--no-cfg` has no defined meaning in this RFD and is rejected at parse time.
- Add a manual `clap::FromArgMatches` impl that merges the `-c` and `-C` vectors
  into a single `Vec<CfgDirective>` preserving command-line order via
  `ArgMatches::indices_of(..)`.
  Same pattern as `ToolDirectives` in `query.rs`.

**Pipeline integration**:

- Phase 1 of the config pipeline (`partial_without_conversation`) skips every
  `CfgDirective::Revert` entry — no claim history is available before
  conversation resolution.
  `conversation.default_id` is derived from `Apply` directives only.
- Phase 2 of the config pipeline runs the full directive loop with both `Apply`
  and `Revert` directives, using the folded current claims state from `init` +
  event-stream deltas as the starting point.

**Revert implementation**:

- **File-based `-C` (identity-scoped)**: compute the target identity set from
  every applicable hash per candidate file — workspace files always contribute
  their workspace-relative path hash (and `hash(id)` if declared),
  user-local/user-workspace files contribute the absolute resolved path (and
  `hash(id)` if declared and the file is present).
  Workspace files remain targetable post-deletion because the path hash is
  derivable without the file; non-workspace deleted files yield an empty
  identity set and are a no-op.
  Scope = `{field | claims_state[field] has any identity in target_set}`.
  Walk deltas backward past all claims whose identity list intersects the target
  set, stopping at the first claim with no target-matching identity (or base).
- **Key-value `-C` (value-scoped)**: read the current resolved value of the
  field, compare to the specified value, skip on mismatch.
  Walk deltas backward past all claims on the field where the resolved value was
  still the target, stop at the first different state (or base).
- **JSON-object `-C`**: pre-expand via `try_merge_object` into per-leaf kv
  reverts, run the kv-revert algorithm for each leaf independently.
- Emit diagnostics for unmatched `-C` invocations (see [Missing
  claims](#missing-claims-ux-and-diagnostics)).
- The starting claims state for phase 2 includes env unclaims (see [Current
  claims state](#current-claims-state)) so env-set fields are protected from
  file-based `-C`.

**Integration tests** covering all worked examples from this RFD:

- Basic cross-invocation revert
- Overlapping sources with identical values
- Shortcut flag override
- Key-value revert (matching and non-matching values)
- JSON-object revert (expansion equivalence with kv form)
- Repeated claimant across invocations (A → B → A)
- Source file edited between invocations (claim-history guarantee)
- Missing-claims diagnostic emission

Depends on Phase 3.

## References

- [RFD 008]: Ordered Tool Directives — establishes left-to-right processing for
  interleaved CLI flags.
- [RFD 035]: Multi-Root Config Load Path Resolution — defines the three-root
  search for `--cfg` paths, which `-C` reuses.
- **Conversation-ID inheritance** (`--cfg=jp-c<id>` and `--fork` implicit
  config) is defined in a future RFD.
  This RFD assumes inherited conversation partials claim every field they set
  under `hash(conversation_id)`, collapsing inner provenance.
- [RFD 054]: Split Conversation Config and Events — separates `base_config`
  into its own file.
  This RFD reshapes that file to carry both the workspace snapshot and
  creation-time per-directive deltas, preserving the cleanliness of
  `events.json`.
- [RFD 060]: Config Explain — future RFD that may benefit from provenance data.
- `crates/jp_conversation/src/stream.rs` — `ConfigDelta` struct and
  conversation stream.
- `crates/jp_cli/src/config_pipeline.rs` — `ConfigPipeline` and
  `apply_cfg_args`.
- `crates/jp_cli/src/lib.rs` — `KeyValueOrPath` enum and `-c`/`--cfg` parsing.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 035]: 035-multi-root-config-load-path-resolution.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 060]: 060-config-explain.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 079]: 079-config-sources-and-load-order.md
