# RFD 070: Negative Config Deltas

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-03
- **Required by**: [RFD 078]
- **Summary**: Adds `-C` / `--no-cfg` to revert previously applied configuration
  sources using per-field provenance claims recorded on `ConfigDelta`.

## Summary

This RFD introduces `-C` / `--no-cfg`, the negative counterpart to `-c` /
`--cfg`, to revert a configuration source's changes in a conversation.
Each `ConfigDelta` records a per-field claims map tracking which source last set
each field.
Reverting a file, `key=value` assignment, JSON object, or shortcut flag clears
only the fields still claimed by that source and leaves later overrides intact.

## Motivation

Users can layer config files onto a conversation across invocations:

```sh
jp query -n -c dev       # new conversation with dev overrides
jp query -c architect    # add architect overrides on top
```

However, there is no way to remove a previously applied config's influence.
To stop using `dev` without starting a new conversation, the user must manually
identify every field `dev` set and override each one with `--cfg key=value`.

The expected workflow is:

```sh
jp query -C dev          # "undo" dev's overrides
```

This command reverts fields that `dev` introduced while leaving intact any field
subsequently claimed by another config source, such as `architect`.
If both `dev` and `architect` set `tools = [read_file]`, reverting `dev` must
not disable the tool because `architect` still enables it.

Without `-C`, users must either track config state manually or start a new
conversation whenever they change config profiles.

## Design

### CLI surface

`-C` / `--no-cfg` is a global flag that accepts the same input syntax as `-c` /
`--cfg`: config file paths, `key=value` assignments, and JSON object literals:

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

All forms of `-C` use the conversation's claim history, but they scope the
revert differently by form:

- **File-based** (`-C dev`) scopes by **source identity**: the hash of the
  file's [`id`](#stable-identity-via-id), or the hash of its resolved path when
  `id` is absent.
  Multi-root resolution ([RFD 035]) produces one hash per resolved file.
  Only each file's top-level `id` field is read from disk; the full partial is
  not loaded to compute revert scope.
- **Key-value** (`-C foo=BAR`) scopes by **current value**: `-C` reverts `foo`
  only when its current resolved value equals `BAR`, walking back past all
  claims on `foo` (regardless of source) to the prior different value.
  See [Revert algorithm](#revert-algorithm).
- **JSON object** (`-C '{...}'`) expands via `try_merge_object` (the same path
  `-c` uses for JSON input) into per-leaf key-value reverts, running each leaf
  independently.
- **Shortcut flags** have no dedicated `-C` flag; revert them with the
  equivalent key-value form (for example, `-C assistant.model.id=foo` to undo
  `--model foo`).

File-based revert scope comes from claim history, not from the file's current
contents.
`-C dev` reverts every field `dev` claimed in the conversation's history, even
if `dev.toml` has since been edited to remove those fields.

Each claim stores one or more identity hashes for its source.
A workspace file's claim always includes the workspace-relative path hash, plus
`hash(id)` when the file declares `id`; matching either hash is sufficient at
revert time, so workspace files remain targetable after deletion.
User-local and user-workspace files use scan-based resolution at revert time and
cannot be targeted by path after deletion.
See [Source identity](#source-identity) for the per-root rules.

### Processing model

Negative and positive args are processed together from left to right, following
the ordered-directive model in [RFD 008].
Each `-C` / `--no-cfg` operates on the state accumulated up to its position in
the argument list:

```sh
# Left-to-right: apply dev, then revert dev (net effect: no dev)
jp query -c dev -C dev

# Left-to-right: revert dev first (no-op if not claimed), then apply architect
jp query -C dev -c architect
```

#### Phase boundaries

JP's config pipeline runs in two phases ([RFD 079]):

- **Phase 1** (`partial_without_conversation()`) runs before a conversation is
  loaded and processes `-c` directives to resolve `conversation.default_id`.
- **Phase 2** runs after the conversation is loaded and applies the full
  pipeline, including the per-conversation layer.

Because `-C` needs the conversation's claim history to compute its scope, phase
1 skips `CfgDirective::Revert` entries and processes only `Apply` directives.
As a result, `-C` cannot affect which conversation is loaded.
In practice, `conversation.default_id` is set in workspace config or session
state rather than reverted mid-sequence with `-C`.

### Data model changes

#### `CfgDirective` wrapper

`KeyValueOrPath` is unchanged.
A wrapper enum records whether an argument applies or reverts a config layer:

```rust
/// A config layer directive: apply or revert.
enum CfgDirective {
    /// Merge this config on top of the current state.
    Apply(KeyValueOrPath),
    /// Revert fields that match this config.
    Revert(KeyValueOrPath),
}
```

`-c` produces `CfgDirective::Apply` and `-C` produces `CfgDirective::Revert`.
Both share `KeyValueOrPath` resolution, and the interleaved list preserves
left-to-right order.

`ResolvedCfgArg::Partials` carries a `SourceId` alongside each
`PartialAppConfig` so the claims pipeline knows which file set each field:

```rust
enum ResolvedCfgArg {
    KeyValue(KvAssignment),
    Partials(Vec<(PartialAppConfig, SourceId)>),
}
```

`ResolvedCfgDirective` preserves the polarity through resolution:

```rust
enum ResolvedCfgDirective {
    Apply(ResolvedCfgArg),
    Revert(ResolvedCfgArg),
}
```

Polarity (apply vs. revert) is orthogonal to the resolved payload (key-value vs.
file partials).
`apply_cfg_args` dispatches on both:

- `Apply(*)` merges the config and records claims (file identity for `Partials`,
  key-value identity for `KeyValue`).
- `Revert(Partials(...))` is **identity-scoped**: it reverts fields whose
  current claim matches the target file identity set.
- `Revert(KeyValue(...))` is **value-scoped**: it reverts the field if its
  current resolved value matches the target value, walking back past all claims
  on that field.

#### Claims on `ConfigDelta`

`ConfigDelta` (`crates/jp_conversation/src/stream.rs`) gains two fields for
field clearing and per-field provenance:

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

- **`delta`**: the config diff.

- **`unsets`**: dotted field paths to reset to `None`.
  Revert deltas populate `unsets` for every field in their revert scope, and
  normal deltas can also use it to clear fields.
  `apply_config_delta` applies `unsets` **before merging `delta`**: each path in
  `unsets` is reset to `None`, and `delta` is then merged on top.
  Resetting before merging gives revert deltas replace semantics on custom merge
  types without per-type handling (see [Revert semantics for custom merge
  types](#revert-semantics-for-custom-merge-types)).

  `PartialAppConfig` and its nested partial types gain an `unset(path: &str)`
  method that mirrors `AssignKeyValue` dispatch, setting the target field to
  `None`.
  For set-like and identity-bearing vec fields (see [Claim
  granularity](#claim-granularity)), `remove_element(path: &str, identity:
  &str)` filters the array by matching each element's `ClaimIdentity`.
  Duplicate-capable vec fields use whole-field revert via `unset(path)` followed
  by assigning the prior vector.
  All operations run directly on the Rust partial types, avoiding JSON
  round-trips across custom serde implementations such as `MergeableVec` and
  `MergedString`.

- **`claims`**: maps each field path to a list of `"HASH:LABEL"` entries, with
  three states:

  - **Absent from the map**: this delta did not claim the field.
  - **Empty `Vec`**: explicitly unclaimed, used for environment variable
    overrides (see [Key-value and shortcut flag
    claims](#key-value-and-shortcut-flag-claims)).
  - **One or more entries**: claimed by the listed identities.
    A file can record multiple identity hashes (for example, both `hash(id)` and
    `hash(path)` for a workspace file with `id`), and a revert matches if any
    stored `HASH` prefix matches.
    `LABEL` is for display only.

#### Source identity

Each claim entry is formatted as `HASH:LABEL`.
The hash (for example, SHA-256) is used for identity matching.
The label provides human-readable context for provenance display ([RFD 060]) and
redacts user-specific paths so they are not written to shared workspace storage:

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
| Environment variable            | None                       | Explicit unclaim (empty `Vec`)     |

A source can record multiple identity hashes on a claim, and a revert matches if
any stored hash is in the target set:

- **Workspace file**: always hashes the workspace-relative path (such as
  `.jp/config/skill/dev.toml`), plus `id` when declared.
  Because the workspace-relative path can be derived without reading the file,
  deleted workspace files remain targetable by `-C`.
- **User-workspace and user-local files**: hash the absolute resolved path, plus
  `id` when declared.
  Revert scans the config directory to discover candidate files, so a deleted
  file cannot be discovered by scan.

Renaming a file without `id` changes its path hash; files that may be renamed
should declare `id`.

Because conversation streams live in the workspace and are often committed to
version control, workspace-relative paths are stored verbatim while
user-workspace and user-local paths use placeholder labels.
`config explain` ([RFD 060]) can still resolve a placeholder label on the local
machine by hashing known config files and matching the stored hash.

#### Stable identity via `id`

Config files can declare an optional top-level `id` field:

```toml
# .jp/config/skill/dev.toml
id = "dev-persona"

[assistant]
name = "DevBot"
```

When `id` is set, claims include `hash(id)` so identity survives file renames:
renaming `dev.toml` to `developer.toml` while keeping `id = "dev-cfg"` lets `-C
developer` match claims recorded under the old filename.

Files with the same `id` share a config identity, allowing a personal override
and a team config with the same `id` to replace each other cleanly.

`AppConfig` gains an optional `id` field (like the existing `inherit` field),
and `#[derive(Config)]` generates the corresponding `Option<String>` on
`PartialAppConfig`.
`--cfg` resolution reads `id` to compute the source identity, and merging resets
it to `None` so it does not appear in the resolved `AppConfig` or persisted
config deltas.

### Claims lifecycle

#### Building claims during `-c`

When the pipeline processes a `-c` arg, it merges the partial into the
accumulated config and records claims for each field the partial sets:

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

Key-value assignments record their own claim under a distinct key-value source
identity.
Key-value `-C` does not use this identity for matching (see [Key-value
revert](#key-value-revert-c-foo-bar)); instead, the claim lets a later
file-based `-C <source>` see that the field was overridden by the user and skip
it.
See [Key-value and shortcut flag claims](#key-value-and-shortcut-flag-claims)
for the identity format.

A single `-c dev` can resolve to multiple files across config roots ([RFD 035]):
user-global, workspace, and user-workspace.
Each resolved file records its own source identity and claims.
Files merge in precedence order (user-global < workspace < user-workspace), so
when two files set the same field, the higher-precedence file's claim overwrites
the lower one.
Files that declare the same `id` hash to the same identity regardless of root.

Within a single invocation, later `-c` args overwrite earlier claims for the
same field, matching left-to-right merge order.

#### Key-value and shortcut flag claims

Both `-c key=value` and CLI shortcut flags (`--model`, `--reasoning`, etc.)
record claims with a uniform key-value identity:

```txt
kv_identity(field_path, canonical_value) =
    hash("kv:" + field_path + "=" + canonical_value)
```

`canonical_value` is the field's typed value serialized in its standard serde
form after transformations such as model alias resolution.
For example, `--model opus` (where `opus` resolves to
`anthropic/claude-opus-4-6`) and `-c
assistant.model.id=anthropic/claude-opus-4-6` both produce
`hash("kv:assistant.model.id=anthropic/claude-opus-4-6")`.

This identity is used only when recording `Apply` claims, not when matching
`-C`.
Key-value `-C foo=BAR` is value-based (see [Key-value
revert](#key-value-revert-c-foo-bar)), while the recorded claim prevents a later
`-C <source>` from reverting a field that was pinned by `-c
assistant.model.id=something`.

Shortcut flags record claims by threading `claims: &mut BTreeMap<String,
Vec<String>>` through `IntoPartialAppConfig::apply_cli_config` and each
`apply_*` helper.
Each helper registers `(field_path, canonical_value)` alongside its partial
mutation:

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

Keeping the `&'static str` field path next to the assignment avoids a separate
flag-to-field mapping table, which would not capture helpers that branch on
variants (`apply_editor`) or set multiple fields (`apply_reasoning`).

Environment variable overrides (`JP_CFG_*`) record `claims[path] = vec![]` (an
explicit unclaim).
Environment variables are ambient session state rather than per-invocation
directives: users undo an environment override by unsetting the variable in
their shell, and the empty claim entry prevents a later file-based `-C source`
from reverting an environment-set field.

#### Persisting claims: per-directive deltas

Instead of persisting one `ConfigDelta` per invocation, JP persists one
`ConfigDelta` per apply or revert step:

- Each `-c` directive (file or key-value) emits a delta with that directive's
  diff and claims.
- Each `-C` directive emits a delta with the reverted values, updated claims,
  and any `unsets`.
- All CLI shortcut flags (`--model`, `--reasoning`, etc.) batch into a single
  trailing delta after all `-c` and `-C` directives.
  Because each shortcut flag touches a distinct set of fields, batching them
  does not lose intermediate claims.

A `ConfigDelta` is emitted whenever `delta`, `claims`, or `unsets` is non-empty.
Even when the value diff is empty (for example, when `-c architect` sets a field
to the same value `dev` already set), the claims-only delta is persisted so
provenance stays up to date.

**Why per-directive.** Consider layering two configs in one invocation and
reverting one in the next:

```sh
jp query -c dev -c architect       # invocation 1: layer dev then architect
jp query -C architect              # invocation 2: drop architect
```

With one delta per invocation, invocation 1's claims map would record only the
final state (`architect` on every shared field), losing `dev`'s intermediate
claim.
In invocation 2, `-C architect` would walk back past `architect`'s claim
straight to the base config, erasing `dev`'s values on shared fields.

With per-directive deltas, invocation 1 persists delta A (`dev`'s values and
claims) followed by delta B (`architect`'s values and claims).
In invocation 2, walking back past `architect`'s claim lands on delta A,
restoring `dev`'s values whether `dev` and `architect` were applied in one
invocation or across two.

The trade-off is one persisted event per directive instead of one per invocation
(see [Drawbacks](#drawbacks)).

**Legacy backward compatibility.** Existing conversation streams without claims
deserialize with an empty map (`#[serde(default)]`).
When a field has no claim entry, file-based `-C` skips it rather than guessing
from value diffs, so `-C` is a no-op on pre-existing conversations.

#### Delta plumbing changes

`ConfigDelta` uses custom serde and helper functions that ignore unknown fields,
so adding `#[serde(default)]` to `claims` and `unsets` on the struct is not
enough.
Three changes are required in `crates/jp_conversation/src/stream.rs`:

- **`deserialize_config_delta`** (around line 158) reads only `delta` and
  `timestamp` from a raw `serde_json::Value`, bypassing derived deserialization.
  Extend it to read `claims` and `unsets` explicitly.
- **`add_config_delta`** (around line 302) destructures `{ delta, timestamp }`
  and skips emission when the recomputed diff is empty.
  Update it to preserve `claims` and `unsets` across diff recomputation, and to
  emit the delta whenever `delta`, `claims`, or `unsets` is non-empty.
- **Single `apply_config_delta` helper.** Six call sites replay deltas by
  calling `partial.merge(&(), delta.into())` directly: `config()`,
  `IntoIter::next` and `next_back`, `Iter::next` and `next_back`, and
  `IterMut::next`.
  Replace all six with `apply_config_delta(partial: &mut PartialAppConfig,
  delta: &ConfigDelta)`, which applies `delta.unsets` first and then merges
  `delta.delta`.
  Applying `unsets` before `delta` is required for revert semantics on custom
  merge types (see [Revert semantics for custom merge
  types](#revert-semantics-for-custom-merge-types)) and keeps `config()` aligned
  with the per-event iterators.

#### Current claims state

Before running an invocation's directive loop, the pipeline builds the **current
claims state**, a `BTreeMap<String, Vec<String>>` mapping each field to its
current claiming identities (absent = unclaimed, empty `Vec` = explicitly
unclaimed, non-empty = claimed by the listed identities):

1. **Fold persisted claim history.** Start from empty claims on `base`, then
   fold each `ConfigDelta` in `base_config.json`'s `init` list followed by each
   `ConfigDelta` in `events.json`, keeping the most recent claim entry per
   field.
2. **Apply current-invocation environment unclaims.** For each field set in the
   env-only partial returned by `PartialAppConfig::from_envs()`
   (`crates/jp_config/src/lib.rs`), set `claims_state[field] = vec![]`.
   Use `from_envs()` rather than `load_envs()` (`util.rs`), which returns the
   base config merged with environment variables rather than only the
   environment-set fields.

Seeding environment unclaims is necessary because `load_base_partial`
(`crates/jp_cli/src/lib.rs`) merges environment variables into the base partial
before `ConfigPipeline` runs.
Without this step, running `jp query -C dev` with `JP_CFG_ASSISTANT_MODEL_ID`
set would walk back to a persisted claim and emit a revert delta that overwrites
the active environment variable.

As the pipeline processes `-c` and `-C` directives from left to right:

- Each `Apply` updates `claims_state` with the claims it records.
- Each `Revert` sets each reverted field in `claims_state` to its post-revert
  owner: the prior non-target claim found during walk-back, or absent if the
  walk reached `base`.
  Because `claims_state` matches the emitted revert delta's `claims` map, later
  directives in the same invocation see the restored owner (for example, in `-c
  dev -c architect -C architect -C dev`, the second `-C` sees `dev` as the
  restored owner of shared fields and reverts them).

Each directive's claims delta is persisted individually (see [Persisting
claims](#persisting-claims-per-directive-deltas)).

### Revert algorithm

`-C` has two scoping modes: **file-based reverts scope by source identity**, and
**key-value reverts scope by current value**.
JSON-object `-C` expands into per-leaf key-value reverts.

#### File-based revert: `-C <source>`

1. **Compute the target identity set.**

   - Resolve `<source>` via `config_load_paths` across roots ([RFD 035]) to one
     or more candidate paths (existing or not).
   - For each candidate, collect its applicable identity hashes:
     - If the file exists and declares a top-level `id`, include `hash(id)`.
     - For a workspace file, include `hash(workspace-relative path)`, which can
       be derived even after the file is deleted.
     - For a user-local or user-workspace file, include `hash(absolute resolved
       path)`, which requires the file to exist on disk because resolution is
       scan-based.
   - The target set is the union of all collected hashes.
     Files sharing an `id` collapse onto `hash(id)`, while path hashes remain
     distinct per path.

2. **Determine field scope from `claims_state`** (including environment
   unclaims):

   - **Non-empty `Vec` with a hash in `target_set`**: owned by the target;
     include in scope.
   - **Non-empty `Vec` with no hash in `target_set`**: owned by another source;
     skip.
   - **Empty `Vec`**: explicitly unclaimed (environment variable override);
     skip.
   - **Absent from `claims_state`**: no provenance recorded; skip.

3. **Compute revert values.** For each field in scope, walk `ConfigDelta` events
   backward past all claims matching `target_set` until reaching either a claim
   with no target hash or `base`.
   Take the field's value at that point.
   If the target value is `None`, add the field path to `unsets` (schematic's
   merge cannot represent a `Some -> None` transition).

4. **Emit a revert `ConfigDelta`** carrying the reverted values in `delta`, any
   cleared field paths in `unsets`, and each reverted field's restored owner in
   `claims` (or omitting fields whose revert reached `base`).

#### Key-value revert: `-C foo=BAR`

1. **Read the current resolved value** of `foo` from the pipeline state (after
   folding persisted deltas and environment variables).
2. **Compare with `canonical(BAR)`.** If the current value differs, skip with a
   diagnostic because the target value is not active.
3. **Walk back** from the most recent `ConfigDelta` to the first state where
   `foo`'s resolved value differed from `BAR` (or to `base`), traversing all
   intervening claims on `foo` regardless of source.
4. **Emit a `ConfigDelta`** setting `foo` to that earlier value (or adding `foo`
   to `unsets` if it was unset) and updating `claims` to the owner at that
   earlier point.

Because key-value `-C` checks only the current value, `-C foo=BAR` undoes `foo =
BAR` whether it came from `-c dev`, `-c foo=BAR`, or a shortcut flag.

#### JSON-object revert: `-C '{...}'`

Expand the JSON object via `try_merge_object` into per-leaf assignments and run
the key-value revert algorithm on each leaf independently.
Leaves whose current value does not match emit a diagnostic, while matching
leaves revert normally.

#### Why claim-history, not current-file-contents

Computing file-revert scope from the target file's current contents on disk
would miss fields that were removed from the file after it was applied, leaving
stale values in the conversation.
Using claim history instead undoes everything the source contributed to the
conversation.

How file deletion behaves depends on the config root:

- **Workspace files** remain targetable after deletion because the
  workspace-relative path is always hashed into the claim and can be derived
  without reading the file.
- **User-local and user-workspace files** cannot be targeted after deletion
  because their candidate paths come from scanning the directory at revert time.
  To keep a file revertable if moved or replaced, declare `id` on it or store it
  in the workspace.

See [RFD 060] for provenance display.

#### Revert semantics for custom merge types

Several config fields use custom merge wrappers whose strategies can be Append,
Prepend, or Replace:

- `MergeableString` (for example, `assistant.system_prompt` in
  `crates/jp_config/src/types/string.rs` and `internal/merge/string.rs`).
- `MergeableVec` (for example, `assistant.instructions` and
  `conversation.attachments` in `crates/jp_config/src/types/vec.rs` and
  `internal/merge/vec.rs`).
- `MergeableMap` (for example, `plugins.command`, `providers.mcp`, and
  `conversation.labels` in `crates/jp_config/src/types/map.rs` and
  `internal/merge/map.rs`).

The same issue occurs on forward deltas for these types, as seen with
`conversation.labels`: `PartialConversationConfig::delta` collects changed
entries into a plain `MergeableMap::Map`, dropping the wrapper's `strategy` and
omitting keys that were removed.
When a layer replaces the map (`strategy = "replace"`), the recorded delta
deep-merges on the next fold and removed entries reappear.
`attachments` (`MergeableVec`) and `conversation.tools` (`IndexMap`) have the
same problem.

Two security-sensitive fields (`conversation.labels`, and `access.fs` /
`access.env` via `rule_delta` in `jp_config::conversation::tool::access`) work
around this by emitting the entire container with `strategy = "replace"`
whenever an entry is removed.
`unsets` replaces those point fixes with a general mechanism for explicit
removal.

Without `unsets`, a revert delta whose target value carried an Append strategy
would append the prior value onto the current state instead of replacing it.
To force replacement, every revert delta records the field path in `unsets` and
the target value in `delta.delta`.
`apply_config_delta` runs `unsets` first (setting the field to `None`) and then
merges `delta.delta`.
In `crates/schematic/src/internal.rs`, `merge_setting(prev: Option<T>, next:
Option<T>, ...)` only calls the custom merger when both sides are `Some`:

```rust
if prev.is_some() && next.is_some() {
    merger(prev.unwrap(), next.unwrap(), context)
} else if next.is_some() {
    Ok(next)
} else {
    Ok(prev)
}
```

When `prev` is `None` and `next` is `Some(target)`, the custom merger
(`string_with_strategy`, `vec_with_strategy`, `map_with_strategy`) is bypassed
and the field is set directly to `target`.
`merge_nested_setting` uses the same `Option` check for nested configs:

- The revert-delta builder needs no per-type helpers to force Replace semantics.
- `ToPartial` can be used as-is for the target value regardless of the field's
  merge strategy.
- Any strategy marker on the stored target value (such as
  `PartialMergeableString::Merged { strategy: Append, ... }`) has no effect:
  revert application sees `prev = None`, and later deltas use their own
  `next.strategy` rather than consulting `prev.strategy`.

### Conversation creation change

Previously, creating a conversation set `base_config` to the fully resolved
`AppConfig`, including environment variables, `-c` args, and CLI flags from the
creation invocation.
Baking those overrides into `base_config` without a `ConfigDelta` or claims map
left a later `-C` with no claims to match.

To preserve the `events.json` layout from [RFD 054] while recording
per-directive provenance on the creating invocation, `base_config.json` is
reshaped to store both the workspace config snapshot and the creating
invocation's per-directive deltas.

#### Pinning is a constraint on the reshaping

A conversation pins its configuration at creation time: upgrading JP or editing
the workspace config does not change how an existing conversation resolves, and
users opt a conversation into the current workspace config explicitly with `-c
WORKSPACE`.

Storing a resolved `AppConfig` enforces pinning by construction.
Resolution materializes `#[setting(default)]` values into concrete fields, so a
default that changes in a later release cannot alter an existing conversation,
and the stored config is guaranteed to be complete and valid.

Storing a `PartialAppConfig` instead would lose both guarantees: unset fields
would take their defaults from whichever binary loads the conversation, and the
type system would no longer guarantee that the stored config is complete.
For that reason, `base` remains a resolved `AppConfig`, but it is resolved from
the workspace files alone rather than from the entire creating invocation.

#### File format

`base_config.json` becomes a JSON object with two keys:

```json
{
  "base": { /* the workspace configuration, partial-encoded */ },
  "init": [
    { /* ConfigDelta — first creation-time source */ },
    { /* ConfigDelta — next source */ }
  ]
}
```

- **`base`** is a resolved `AppConfig` built from the workspace's own config
  files merged via inheritance (`.jp/config.toml` and the `.jp.toml` chain),
  with program defaults materialized into concrete values.
  User-global config, user-workspace config, environment variables, `-c` args,
  and CLI flags are excluded.
- **`init`** is an ordered list of `ConfigDelta` entries, one per source layered
  on top of the workspace files during creation: user-global config,
  user-workspace config, environment variables, each `-c` directive (file,
  `key=value`, or JSON object), each `-C` directive, and the trailing
  shortcut-flags batch.
  Each entry carries its diff, claims map, and `unsets`.

How `base` is serialized on disk does not change: `to_parts()` writes
`base_config.to_partial()`, and `from_parts()` reads it back through
`compat::deserialize_partial_config` followed by `finalize_recovered_config`,
retaining schema-aware stripping of unknown fields and fallback handling for
missing required fields.

##### What an `init` entry holds

Each `init` entry records the diff between resolving the sources up to and
including that source and resolving the sources before it, in normal load order,
rather than the raw partial the source declared.

Recording the post-resolution diff has two consequences:

1. **Indirectly affected fields are captured.** If the workspace sets
   `assistant.model.id = "coder"` with `providers.llm.aliases.coder =
   "openai/gpt-4o"`, and a user-workspace override changes that alias to
   `anthropic/claude-opus-4-6`, `base` already holds the resolved model ID
   `openai/gpt-4o`, where `ModelIdOrAliasConfig::resolve_in_place` is a no-op.
   Replaying only the alias map change would leave `assistant.model.id` on
   `openai/gpt-4o`.
   Diffing the resolved configs includes the updated `assistant.model.id` (as
   well as inquiry inheritance and instruction ordering computed during
   resolution).
2. **Source precedence is preserved.** `load_partial_configs_from_files` loads
   user-global config before workspace config, so `conversation.tools.'*'.run =
   "ask"` in the workspace overrides `"unattended"` in user-global config.
   Replaying user-global's raw partial after `base` would invert that precedence
   and overwrite `"ask"` with `"unattended"`.
   Diffing the resolutions omits any user-global field that the workspace
   overrode.

This preserves the key invariant tested in Phase 3: folding `base` followed by
every `init` entry produces the same `AppConfig` as resolving the entire
creation invocation at once.

Both `base` and `init` are written once at conversation creation and never
modified afterward.
Subsequent `-c` and `-C` invocations append their per-directive deltas to
`events.json`, keeping creation-time config blobs out of `events.json` ([RFD
054]):

```txt
Before:  base_config.json = AppConfig resolved from every source at once
After:   base_config.json = { base: AppConfig resolved from workspace files,
                              init: [creation-time ConfigDeltas] }
         events.json       = post-creation ConfigDeltas + ConversationEvents
```

#### The workspace has to be self-contained

Because workspaces are committed to version control and shared across machines,
`base` must not depend on machine-local config: a model defined only in one
user's global config would fail to resolve when a colleague clones the
repository.

Accordingly, `base` is resolved from workspace config files alone, and
conversation creation fails if those files do not form a valid `AppConfig` on
their own.
With a self-contained workspace config, any contributor who clones the
repository can run `jp query --new hello` (assuming external runtime
dependencies such as API keys, a running `ollama` instance, or tools on `PATH`).

User-global and user-workspace configs represent user-specific overrides rather
than workspace properties, so they are recorded in `init` alongside environment
variables, `-c` args, and CLI flags, with redacted `<user-local>` and
`<user-workspace>` labels (see [Source identity](#source-identity)).
Recording them in `init` still preserves the exact configuration the
conversation's turns ran under when the conversation is shared.

This check runs only when creating a conversation (`jp query --new`); existing
conversations never re-resolve `base` and continue to load unchanged.
Because this check rejects `jp query --new` in workspaces that previously relied
on user-global config or CLI flags to supply required fields, it ships as a
separate change (Phase 2b) before the storage format change in Phase 3.

#### Creation-time resets, and `NONE`

Two invocation shapes cannot separate the creating invocation's overrides into
`init`; in both cases, creation folds the entire invocation into `base` and
writes `init: []`:

1. **`--cfg=NONE` opts out of implicit config.** `ConfigPipeline::new` checks
   for `NONE` before reading any config files and starts from an empty partial
   ([RFD 038]), allowing `NONE` to work even when workspace config is missing or
   broken.
   Without workspace files, an empty partial cannot finalize into a valid
   `AppConfig` because `assistant.model.id` requires `provider` and `name`.
   Therefore, `jp query --new --cfg=NONE --cfg=./mre.toml hello` resolves `base`
   from the whole invocation and skips the self-contained workspace check.
2. **A reset keyword appears in `--cfg`.** [RFD 038] persists `--cfg=WORKSPACE`
   as a `Reset` followed by an `Apply` of the workspace state, and
   `fold_config_delta` implements `Reset` by resetting the accumulated state to
   an empty partial.
   Placing a `Reset` inside `init` would discard `base` during replay, causing
   any field not restored by the reset's `Apply` (including
   `#[setting(default)]` fields such as `assistant.request.max_retries`) to fall
   back to the current binary's defaults instead of the pinned values.
   To preserve pinning, creation folds everything up to and including the last
   reset into `base`, and records only directives after the last reset in
   `init`.

In both cases, pinning is preserved at the cost of creation-time provenance:
fields folded into `base` have no claims in `init`, so `-C` is a no-op on them,
matching legacy conversations.

#### API changes

- **`ConversationStream`**: keeps `base_config: Arc<AppConfig>` (resolved from
  workspace files alone) and adds `init: Vec<ConfigDelta>` for the creation-time
  deltas.
  `config()` folds `base_config.to_partial()`, then each `init` delta, then each
  event-stream delta via `apply_config_delta`.
- **`from_parts()` / `to_parts()`**: keep their two-parameter signatures
  (`base_config` JSON value and `events` JSON array), while changing the
  `base_config` JSON value to `{ base, init }`.
  `base` itself still serializes via `base_config.to_partial()` and deserializes
  through `compat::deserialize_partial_config` and `finalize_recovered_config`.
- **Per-event iterators** (`Iter`, `IterMut`, `IntoIter`): fold `base` and
  `init` before iterating over `events` so per-event config views match
  `config()`.
- **`Workspace::create_and_lock_conversation`**: gains an `init_deltas:
  Vec<ConfigDelta>` parameter, populated by `query --new`.
- **Storage layer** (`jp_storage`): `Storage::persist_conversation` writes `{
  base, init }` on creation, and `load_conversation_stream` reads both the new
  and legacy formats (see [Backward
  compat](#backward-compat-for-base-config-json)).
- **Fork** (`crates/jp_cli/src/cmd/conversation/fork.rs`): clones both `base`
  and `init` from the source conversation without re-resolving from disk,
  preserving the source conversation's pinned base and initial claims.

#### Backward compat for `base_config.json`

`load_conversation_stream` supports both the new and legacy formats:

1. Parse `base_config.json` as JSON.
2. If the root object has a `base` key, deserialize `base` as the stored config
   and `init` (defaulting to `[]`) as the creation-time delta list.
3. Otherwise, treat the root object itself as `base` with `init: []`.

Because `base` uses the same partial-encoded format in both versions, legacy
files require no field conversion: their creation-time overrides remain baked
into `base`, and `-C` is a no-op on those fields.

On persist, `Storage::persist_conversation` inspects the existing
`base_config.json`:

- **File absent** (new conversation): write `{ base, init }`.
- **New shape** (`base` key present): copy the file verbatim into the staging
  directory, preserving any manual edits.
- **Legacy flat shape**: rewrite the file once as `{ base: <legacy content>,
  init: [] }` in the staging directory; subsequent persists take the verbatim
  copy path.

Once all active conversations have been persisted under the new shape, the
legacy fallback in `load_conversation_stream` can be removed.

#### User-facing commands

`conversation edit` (`-b` / `--base-config`) and `conversation path`
(`--base-config`) (`crates/jp_cli/src/cmd/conversation/edit.rs` and
`crates/jp_cli/src/cmd/conversation/path.rs`) continue to target
`base_config.json`, which holds both `base` and `init` without needing a
separate `--init-config` flag.

Users who open `base_config.json` in an editor see `{ "base": ..., "init": [...]
}`, where `base` has the same structure as the prior flat file and `init` lists
the creation-time deltas.

### Parsing

`-C` uses the same `KeyValueOrPath::from_str` parser as `-c` and wraps parsed
values in `CfgDirective::Revert`:

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

A bare `--no-cfg` without a value is rejected at parse time.

Positive (`-c`) and negative (`-C`) arguments are merged into a single
`Vec<CfgDirective>` in command-line order.
Because clap does not preserve relative order across separate `Vec` fields, a
manual `clap::FromArgMatches` implementation recovers argument positions via
`ArgMatches::indices_of(..)` and sorts the combined list by index, following the
`ToolDirectives` pattern for `--tool` and `--no-tools` in
`crates/jp_cli/src/cmd/query.rs` ([RFD 008]).

To undo post-creation changes in a conversation, users pass `-C <source>` for
each source to revert; a dedicated reset keyword targeting `base + init` is left
to future work.

### Examples

#### Basic cross-invocation revert

```sh
jp query -n -c dev            # invocation 1
jp query -C dev -c committer  # invocation 2
```

- **Invocation 1** writes `base_config.json` with `base` resolved from workspace
  files and `init` holding one `ConfigDelta` per creation-time source.
  `dev`'s entry in `init` claims `assistant.name` and
  `conversation.tools.read_file.enable` under `hash(dev)`.
- **Invocation 2** resolves `-C dev` to `hash(dev)`, reverts the fields still
  claimed by `dev` (`assistant.name` and `conversation.tools.read_file.enable`),
  and then applies `-c committer` on top.

#### Overlapping sources: same field, same value

```sh
jp query -n -c dev      # invocation 1: tools=[read_file]
jp query -c architect   # invocation 2: tools=[read_file]
jp query -C dev         # invocation 3: revert dev
```

- **Invocation 1** claims `conversation.tools.read_file.enable -> hash(dev)`.
- **Invocation 2** also sets `tools.read_file.enable = true`.
  The field's value does not change, so `delta` is empty for that field, but
  `architect`'s partial sets it and overwrites the claim with
  `conversation.tools.read_file.enable -> hash(architect)`.
- **Invocation 3** runs `-C dev`.
  Because `conversation.tools.read_file.enable` is now claimed by
  `hash(architect)`, `-C dev` skips it and `read_file` stays enabled.

#### Shortcut flag override

```sh
jp query -c dev           # invocation 1: model set by dev
jp query --model gpt-4o   # invocation 2: model overridden
jp query -C dev           # invocation 3: revert dev
```

- **Invocation 1** claims `assistant.model.id -> hash(dev)`.
- **Invocation 2** sets `--model gpt-4o`, recording a delta with the new model
  and overwriting the claim with `assistant.model.id ->
  hash("kv:assistant.model.id=gpt-4o")`.
- **Invocation 3** runs `-C dev`.
  Because `assistant.model.id` is claimed by the key-value override from
  invocation 2 rather than `dev`, `-C dev` skips it and preserves `gpt-4o`.

#### Key-value revert (value-based)

```sh
jp query -c assistant.name=DevBot   # invocation 1
jp query -C assistant.name=DevBot   # invocation 2
```

- **Invocation 1** records a delta with `claims["assistant.name"] =
  vec![hash("kv:assistant.name=DevBot")]` and resolves `assistant.name` to
  `DevBot`.
- **Invocation 2** compares the current resolved value (`DevBot`) against the
  target (`DevBot`).
  Because they match, it walks backward through `ConfigDelta` events until
  finding the first earlier state where `assistant.name` was not `DevBot` (or
  reaching `base`) and emits a revert delta restoring that value.

If invocation 2 instead ran `-C assistant.name=Different`, the current value
(`DevBot`) would not match `Different`, and `-C` would skip the field with a
diagnostic.

#### Key-value undo of a file-set value

```sh
jp query -c dev                      # dev.toml sets assistant.name=DevBot
jp query -C assistant.name=DevBot    # undo the DevBot assignment
```

- **Invocation 1** claims `assistant.name -> hash(dev)` and sets
  `assistant.name` to `DevBot`.
- **Invocation 2** runs the value-based revert `-C assistant.name=DevBot`.
  Because the current value matches `DevBot`, it walks back past `dev`'s delta
  to `base` and restores the base value of `assistant.name`, while leaving
  `dev`'s other claimed fields intact.

#### Within-invocation layering then revert

```sh
jp query -n -c dev -c architect    # invocation 1: layer both
jp query -C architect              # invocation 2: drop architect
```

- **Invocation 1** persists two per-directive deltas:
  - Delta A records `dev`'s fields and claims under `hash(dev)`.
  - Delta B records `architect`'s fields and claims under `hash(architect)`,
    overwriting `dev`'s claims on shared fields.
- **Invocation 2** runs `-C architect` with target set `{hash(architect)}` over
  all fields currently claimed by `architect`:
  - On fields set by both `dev` and `architect`, walking back past delta B stops
    at delta A (`hash(dev)`), restoring `dev`'s value and claim.
  - On fields set only by `architect`, walking back past delta B reaches `base`,
    restoring the base config value.

#### Repeated claimant across invocations (A -> B -> A)

```sh
jp query -n -c dev        # invocation 1: dev sets assistant.name
jp query -c architect     # invocation 2: architect overwrites
jp query -c dev           # invocation 3: dev overwrites again
jp query -C dev           # invocation 4: revert dev
```

- **Invocation 1** (delta 1) claims `assistant.name -> hash(dev)`.
- **Invocation 2** (delta 2) claims `assistant.name -> hash(architect)`.
- **Invocation 3** (delta 3) claims `assistant.name -> hash(dev)`.
- **Invocation 4** runs `-C dev`:
  1. Delta 3's claim on `assistant.name` is `hash(dev)`, so the field is in
     scope.
  2. Walking backward past `hash(dev)` claims stops at delta 2, whose claim is
     `hash(architect)`.
  3. `assistant.name` reverts to delta 2's value (`architect`'s value) rather
     than `base`.

Because each directive persists its own delta, `jp query -c dev -c architect -c
dev` in a single invocation produces the same history as invocations 1-3 above.

#### Source file edited between invocations

```sh
jp query -n -c dev           # invocation 1: dev.toml sets assistant.name and model
# <user edits dev.toml, removes the assistant.name setting>
jp query -C dev              # invocation 2: revert dev
```

- **Invocation 1** claims `assistant.name -> hash(dev)` and `assistant.model.id
  -> hash(dev)`.
- **Invocation 2** computes `hash(dev)` (which is unchanged by content edits),
  finds both `assistant.name` and `assistant.model.id` still claimed by `dev` in
  `claims_state`, and reverts both, even though `dev.toml` no longer sets
  `assistant.name`.

## Drawbacks

- **Storage overhead per delta.** Each `ConfigDelta` stores a `BTreeMap` of
  field paths to source hashes (a few hundred bytes for a typical persona file
  setting 10-20 fields).
- **More events per invocation.** Persisting one `ConfigDelta` per `-c` or `-C`
  directive (plus one for shortcut flags) writes 3-4 events for an invocation
  with 2-3 config args instead of 1.
  Each delta is small, but `events_count` (which counts all entries in
  `events.json`; see [RFD 054]) grows faster until UIs switch to turn-based
  counts.
- **`base_config.json` format change.** The top-level config in
  `base_config.json` moves under a `base` key alongside `init`.
  Readers must handle both the new `{ base, init }` object and the legacy flat
  format.
  Steady-state persistence cost is unchanged: `base_config.json` is written once
  at creation, and later persists write only `events.json` and `metadata.json`.
- **Broad implementation surface.** Supporting claims touches several layers of
  the config and conversation pipelines:
  - `jp_conversation` delta plumbing (`deserialize_config_delta`,
    `add_config_delta`, and replacing six `partial.merge(..)` sites with
    `apply_config_delta`).
  - `IntoPartialAppConfig::apply_cli_config` and its `apply_*` helpers, which
    gain a `claims: &mut BTreeMap<...>` parameter.
  - `ClaimIdentity` implementations for the 7 set-like and identity-bearing vec
    fields (6 unique element types; see [Claim
    granularity](#claim-granularity)).
  - `ConversationStream` (`init: Vec<ConfigDelta>`), `from_parts()` /
    `to_parts()`, the three stream iterators,
    `Workspace::create_and_lock_conversation`, `jp_storage`, and `conversation
    fork`.
  - Manual `clap::FromArgMatches` ordering for `-c` and `-C`.

## Alternatives

### Snapshot stack (no provenance)

Replacing the single accumulator with a stack of intermediate snapshots and
walking the stack backward by comparing values avoids storing claims, but fails
whenever two sources set the same field to the same value.
If both `dev` and `architect` enable `read_file`, value comparison cannot tell
that `architect` also enabled it, so `-C dev` would disable `read_file`.

### Per-flag ConfigDelta storage without claims

Storing one `ConfigDelta` per `-c` flag without a claims map records finer
history, but when two `-c` flags set the same field to the same value, the
second diff is empty unless provenance is tracked separately.
Per-directive deltas therefore pair with claims rather than replacing them.

### Full provenance tracking (tagged values)

Wrapping every `Option<T>` field in `PartialAppConfig` in a `Tagged<T>` carrying
source metadata would track provenance in the type system, but would require
changing the representation of every config field across `jp_config`.
A sideband claims map on `ConfigDelta` provides the provenance needed for revert
without changing the config types.

## Non-Goals

- **Direct stream editing.** `-C` never removes or mutates prior `ConfigDelta`
  events; the stream stays append-only, and `-C` appends a new revert delta.
- **Provenance display.** Surfacing which source set each field is covered by
  [RFD 060].

## Risks and Open Questions

### File-based and key-value `-C` have different scoping rules

File-based `-C <source>` scopes by **source identity** (fields whose current
claim matches the target's identity set), whereas key-value `-C foo=value`
scopes by **current value** (reverting `foo` if its resolved value equals
`value`, walking back past all claims on `foo` regardless of owner).

This split reflects how each form is used:

- `-C <source>` undoes a config source's influence, so identity matching is the
  right scope.
- `-C foo=value` undoes a specific value assignment, so value matching is the
  right scope; if the current value does not match, the command skips the field
  with a diagnostic.

Notable consequences:

- `-C foo=A` after `-c foo=A -c foo=B` is a no-op because the current value is
  `B`, not `A`.
  Use `-C foo=B` to undo the active value, or `-C <source>` to revert by source
  identity.
- `-C foo=EnvVal` when `JP_CFG_FOO=EnvVal` is set reverts the environment-set
  field because the user named the value explicitly; file-based `-C` still
  respects environment unclaims.
- `-C assistant.name=DevBot` after `-c dev` (where `dev` set
  `assistant.name=DevBot`) reverts `assistant.name` regardless of which source
  set it.

File-based and key-value `-C` compose in left-to-right order within a single
invocation: each directive runs against the `claims_state` accumulated at its
position.

### Claim granularity

Claims are recorded at the **leaf level**.
When `dev` sets `tools.read_file.enable = true` and `architect` sets
`tools.read_file.run = "ask"`, the claims are:

```txt
conversation.tools.read_file.enable → hash(dev)
conversation.tools.read_file.run    → hash(architect)
```

`-C dev` reverts only `enable`, leaving `architect`'s `run` setting intact.
Entry-level claims (`conversation.tools.read_file`) would revert the entire tool
config, and map-level claims (`conversation.tools`) would let `architect`
enabling `write_file` overwrite `dev`'s claim on the entire map.

Leaf paths are collected by a typed walker over `PartialAppConfig` and its
nested partial types rather than a generic serde-JSON walk, so vec elements can
dispatch to their `ClaimIdentity` implementation.
Each partial type implements `field_paths_into(&self, prefix: &str, out: &mut
Vec<String>)`, mirroring `AssignKeyValue` dispatch.

#### Vec claim granularity: set-like, identity-bearing, duplicate-capable

Vec fields cannot use positional indices for claims because removing an element
shifts subsequent indices.
Each vec field falls into one of three categories:

| Category              | Claim granularity                                                                                   | Revert mechanic                                            |
| --------------------- | --------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| **Set-like**          | Per-element via `ClaimIdentity`                                                                     | `remove_element(path, identity)` filters matching elements |
| **Identity-bearing**  | Per-element via `ClaimIdentity` with `json_identity` fallback when the natural identifier is absent | Same as set-like                                           |
| **Duplicate-capable** | Whole-field claim (path only, no element suffix)                                                    | Revert emits the entire prior value of the field           |

`Vec<String>` fields such as `ToolCommandConfig.args` allow duplicate elements
(for example, `args = ["-I", "path1", "-I", "path2"]`).
A per-element identity would make both `"-I"` entries indistinguishable, so
`remove_element(path, "-I")` would remove both.
Using a whole-field claim avoids that ambiguity: `dev` claims `command.args` as
a unit, `architect` overwrites that claim as a unit, and `-C architect` restores
`dev`'s complete vector.

##### `ClaimIdentity` trait

Set-like and identity-bearing element types implement `ClaimIdentity`:

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

Types without a natural identifier can fall back to `json_identity`:

```rust
pub fn json_identity<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}
```

##### Per-field classification

`AppConfig` has 15 `Vec`-typed fields:

| Field path                                              | Element type            | Category          | Identity strategy                                                 |
| ------------------------------------------------------- | ----------------------- | ----------------- | ----------------------------------------------------------------- |
| `config_load_paths`                                     | `RelativePathBuf`       | Set-like          | `as_str()`                                                        |
| `extends`                                               | `ExtendingRelativePath` | Set-like          | `AsRef::<RelativePath>::as_ref().as_str()`                        |
| `conversation.attachments`                              | `AttachmentConfig`      | Set-like          | `to_url().map(\|u\| u.to_string()).unwrap_or_else(json_identity)` |
| `providers.llm.anthropic.beta_headers`                  | `String`                | Set-like          | identity (self), deduplicated by `vec_dedup`                      |
| `assistant.system_prompt_sections`                      | `SectionConfig`         | Identity-bearing  | `tag.clone().or(title.clone()).unwrap_or_else(json_identity)`     |
| `conversation.inquiry.assistant.system_prompt_sections` | `SectionConfig`         | Identity-bearing  | same as above                                                     |
| `assistant.instructions`                                | `InstructionsConfig`    | Identity-bearing  | `title.clone().unwrap_or_else(json_identity)`                     |
| `assistant.instructions.*.examples`                     | `ExampleConfig`         | Duplicate-capable | whole-field claim                                                 |
| `assistant.instructions.*.items`                        | `String`                | Duplicate-capable | whole-field claim                                                 |
| `assistant.model.parameters.stop_words`                 | `String`                | Duplicate-capable | whole-field claim                                                 |
| `conversation.tools.*.command.args`                     | `String`                | Duplicate-capable | whole-field claim                                                 |
| `conversation.tools.*.enumeration`                      | `serde_json::Value`     | Duplicate-capable | whole-field claim                                                 |
| `editor.envs`                                           | `String`                | Duplicate-capable | whole-field claim                                                 |
| `providers.mcp.arguments`                               | `String`                | Duplicate-capable | whole-field claim                                                 |
| `providers.mcp.variables`                               | `String`                | Duplicate-capable | whole-field claim                                                 |

`IndexMap<String, T>` fields use their map keys as natural identities and need
no `ClaimIdentity` implementation.
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

Set-like and identity-bearing types implement `ClaimIdentity` directly:

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

Set-like and identity-bearing fields record per-element claim paths:

```txt
conversation.attachments[foo.rs]            → hash(dev)
conversation.attachments[/abs/path/bar.rs]  → hash(dev)
config_load_paths[.jp/agents]               → hash(architect)
assistant.instructions[Rust]                → hash(dev)
```

Duplicate-capable fields record path-only claims:

```txt
conversation.tools.fs_read.command.args     → hash(dev)
providers.mcp.arguments                     → hash(workspace)
editor.envs                                 → hash(user-global)
```

##### Revert mechanics by category

- **Set-like and identity-bearing**: revert adds each removed element to
  `unsets` as `path[identity]`, and stream replay calls `remove_element(path,
  identity)` to filter out elements matching `claim_identity()`.
  Before removing an element, the walk-back checks whether the element was
  already present before the target source first claimed it; if so, the element
  is kept.
- **Duplicate-capable**: revert treats the vector as an atomic value.
  Walk-back finds the vector's state at the prior non-target claim (or `base`),
  and the revert delta replaces the entire field (adding `path` to `unsets` and
  writing the prior vector in `delta`, or leaving `delta` empty if the field was
  unset).
  Because duplicate-capable fields are claimed as a unit, reverting `architect`
  when both `dev` and `architect` modified the field restores `dev`'s full
  vector.

### Conversation-ID inheritance

Conversation-ID inheritance (`--cfg jp-c<id>`, defined in a future RFD) expands
another conversation's resolved config into a partial, claims every field it
sets under `hash(conversation_id)`, and persists as a `ConfigDelta` event.
Because the single `hash(conversation_id)` claims every inherited field, the
source conversation's per-source provenance (such as which fields came from
`dev` or `architect`) is collapsed in the target conversation, and `-C jp-c<id>`
reverts the inherited config as a unit.

### Extends sub-files

When `-c dev` loads `dev.toml`, `load_partial_at_path` folds its `extends`
targets into a single `PartialAppConfig` before returning.
Consequently, fields from extended files are claimed under the parent file's
identity (`hash(dev)`), and `-C dev` reverts them together with the rest of
`dev.toml`.
Sub-file provenance for display belongs in `config explain` ([RFD 060]) rather
than in the claims map.

### Claims map size for large config files

The claims map scales with the number of leaf fields a config source sets (10-30
fields for a typical persona file, and bounded at the upper end by the number of
fields in `AppConfig`).

### Renamed or removed config fields

When a config field is renamed or removed in a newer JP version, older claims on
the obsolete field path remain in history as inert entries; `-C` queries only
current field paths and ignores unknown keys.

### Missing claims: UX and diagnostics

When `-C <target>` matches no fields, the command still succeeds and emits a
diagnostic:

- **Legacy conversation with file-based `-C <source>`**: pre-existing
  `ConfigDelta` events have no `claims`, so `claims_state` is empty and
  file-based `-C` is a no-op.
  Key-value and JSON-object `-C` still work on legacy conversations when a
  post-creation delta modified the target field.
- **Unapplied source**: `-C dev` when `dev` was never applied to the
  conversation.
- **Mismatched key-value target**: `-C foo=A` when `foo`'s current resolved
  value is not `A`.
- **Deleted non-workspace source**: `-C dev` when a user-local or user-workspace
  `dev.toml` has been deleted and cannot be found by directory scan.

Diagnostic examples:

```
No fields currently claimed by 'dev' in this conversation.
assistant.name is currently 'Other', not 'DevBot'.
Cannot resolve 'dev' for revert: dev.toml is missing and its identity requires reading the file.
```

Users can inspect current claims in `events.json` or via a future `jp
conversation show --claims` flag.

### Backward compatibility

Older conversation streams have `ConfigDelta` events without `claims`
(`#[serde(default)]` deserializes an empty map), and `base_config.json` files
containing the entire creation-time resolved config (including environment
variables, `-c` args, and CLI flags).
On the first persist after this change lands, legacy `base_config.json` files
are rewritten in place to `{ base: <legacy content>, init: [] }` (see [Phase
3](#phase-3-conversation-creation-and-base-config-json-shape-change)).
Because those legacy files lack recorded claims, `-C` is a no-op on their
creation-time fields, while conversations created after this change support full
revert.

## Implementation Plan

### Phase 1: Delta plumbing, `ClaimIdentity`, and typed walker

This phase adds the data-model and partial-manipulation primitives before wiring
`-C`:

**`ConfigDelta` changes** in `crates/jp_conversation/src/stream.rs`:

- Add `claims: BTreeMap<String, Vec<String>>` and `unsets: Vec<String>` to
  `ConfigDelta` (empty `Vec` = explicit unclaim; non-empty `Vec` = claimed by
  the listed identities).
- Extend `deserialize_config_delta` to read `claims` and `unsets` from the raw
  `Value`.
- Update `add_config_delta` to preserve `claims` and `unsets` across diff
  recomputation, and to emit a delta whenever `delta`, `claims`, or `unsets` is
  non-empty.
- Add `apply_config_delta(partial: &mut PartialAppConfig, delta: &ConfigDelta)`
  to apply `delta.unsets` and then merge `delta.delta`, replacing all six direct
  `partial.merge(..)` call sites (`config()`, `IntoIter::next`/`next_back`,
  `Iter::next`/`next_back`, and `IterMut::next`).

**Config-layer changes** in `jp_config`:

- Add the optional `id` field to `AppConfig` (and the generated `Option<String>`
  on `PartialAppConfig`), stripping it during config resolution so it feeds only
  source identity.
- Implement source hash computation (`hash(id)` when present, plus the resolved
  or workspace-relative path hash).
- Add the `ClaimIdentity` trait and implement it for the 7 set-like and
  identity-bearing vec fields in [Claim granularity](#claim-granularity) (6
  unique element types: `RelativePathBuf`, `ExtendingRelativePath`,
  `AttachmentConfig`, `SectionConfig`, `InstructionsConfig`, and `String`).
  Duplicate-capable vec fields use whole-field claims instead.
- Add `field_paths_into(&self, prefix, out)` on each partial type, descending
  into `Vec<T: ClaimIdentity>` and `IndexMap<String, T>` fields.
- Add `unset(path)` and `remove_element(path, identity)` on `PartialAppConfig`
  and nested partial types.

Tests:

- `claims` and `unsets` round-trip through `deserialize_config_delta` with
  stable `BTreeMap` key ordering.
- Backward-compatible deserialization of legacy `ConfigDelta` events without
  `claims` or `unsets`.
- `unset` and `remove_element` on scalar optionals, nested structs, and vec
  elements.
- `claim_identity()` stability for all 6 element types.
- `apply_config_delta` consistency across `config()` and the three stream
  iterators when `unsets` is populated.

This phase can be merged independently.

### Phase 2: Claims recording and per-directive delta emission

Update `apply_cfg_args` and `apply_cli_config` to record claims and emit
per-directive deltas.

**Claim recording rules:**

- `-c` file args enumerate set leaf paths via `field_paths_into` and record
  claims under the file's source identities (`id` and path hashes).
- `-c` key-value args record claims under `hash("kv:" + field_path + "=" +
  canonical_value)` after type parsing and alias resolution.
- `-c` JSON-object args (such as `-c '{"assistant":{"name":"x"}}'`) expand via
  `try_merge_object` into per-leaf key-value assignments, each recording its own
  key-value identity.
- Shortcut flags pass `claims: &mut BTreeMap<String, Vec<String>>` through each
  `apply_*` helper to record `(field_path, canonical_value)` alongside the
  partial mutation.
- Environment variable overrides (`JP_CFG_*`) record `claims[path] = vec![]` to
  seed the starting claims state for phase 2 (see [Current claims
  state](#current-claims-state)).

**Per-directive delta emission:**

- `apply_cfg_args` snapshots the partial before each `-c` or `-C` directive and
  emits a `ConfigDelta` with that directive's incremental diff, claims, and
  `unsets`, replacing the single-shot `get_config_delta_from_cli`.
- Shortcut flags batch into a single trailing `ConfigDelta` emitted after all
  `-c` and `-C` directives.
- Each emitted delta is persisted via `add_config_delta`.

Tests:

- Claims are recorded for files, key-value assignments, JSON objects, shortcut
  flags, and environment variables.
- `-c dev -c architect` emits two `ConfigDelta` events rather than one.
- Within-invocation `A -> B -> A` persists three deltas with intermediate claims
  preserved.
- Shortcut flags batch into a single trailing delta, and `--model opus` produces
  the same claim identity as `-c assistant.model.id=anthropic/claude-opus-4-6`.
- JSON-object `-c` produces the same leaf claims as equivalent `-c key=value`
  args.
- Environment variables populate `vec![]` in the starting claims state.

Depends on Phase 1 and can be merged independently.

### Phase 2b: Require a workspace to be self-contained

Ships separately before Phase 3 because it rejects `jp query --new` in
workspaces whose config files are incomplete on their own (see [The workspace
has to be self-contained](#the-workspace-has-to-be-self-contained)).

Conversation creation resolves the workspace's own config files
(`.jp/config.toml` and the `.jp.toml` chain) without user-global config,
user-workspace config, environment variables, `-c` arguments, or CLI flags, and
fails if they do not form a valid `AppConfig`.
Invocations with `--cfg=NONE` skip this check (see [Creation-time resets, and
`NONE`](#creation-time-resets-and-none)).

When validation fails, the error names the missing field and states that
user-global config, user-workspace config, environment variables, and CLI flags
do not satisfy workspace config requirements, so the user understands why the
error occurs even when a flag or environment variable supplies the value.

Tests:

- A workspace whose own config files configure a model creates a conversation.
- A workspace whose own config files omit a model fails `jp query --new`, even
  when user-global config, an environment variable, or `--model` supplies one.
- The error message names the missing field and states which sources are
  excluded.
- `jp query` on an existing conversation still succeeds in a workspace that
  would fail `jp query --new`.
- `jp query --new --cfg=NONE --cfg=./mre.toml hello` succeeds even when
  workspace config is missing or incomplete.

### Phase 3: Conversation creation and `base_config.json` shape change

Reshapes `base_config.json` into `{ base, init }` and resolves `base` from
workspace config files alone (see [Pinning is a constraint on the
reshaping](#pinning-is-a-constraint-on-the-reshaping) and [What an `init` entry
holds](#what-an-init-entry-holds)).

- **`ConversationStream`** (`crates/jp_conversation/src/stream.rs`): add `init:
  Vec<ConfigDelta>` between `base_config: Arc<AppConfig>` and `events`.
  Update `config()`, `Iter`, `IterMut`, and `IntoIter` to fold
  `base_config.to_partial()`, then each `init` delta, then each event delta via
  `apply_config_delta`.
  Update `from_parts()` and `to_parts()` to serialize and deserialize the
  `base_config` JSON value as `{ base, init }`.
- **Conversation creation** (`jp_workspace` and
  `crates/jp_cli/src/cmd/query.rs`): `Workspace::create_and_lock_conversation`
  and `create_and_lock_conversation_with_id` take `init: Vec<ConfigDelta>`.
  For normal invocations, `base` is resolved from workspace config files and
  `init` holds the per-source diffs (user-global, user-workspace, environment,
  `-c` and `-C` directives, and shortcut flags).
  When `--cfg=NONE` or a reset keyword is present, `base` absorbs the reset and
  `init` holds only directives after the last reset (or is empty for `NONE`).
- **Storage layer** (`jp_storage`): `Storage::persist_conversation` writes `{
  base, init }` for new conversations, copies `base_config.json` verbatim when
  it already has a `base` key, and rewrites legacy flat files once as `{ base:
  <legacy>, init: [] }`.
  `load_conversation_stream` also wraps legacy flat files in memory on read.
- **Fork path** (`crates/jp_cli/src/cmd/conversation/fork.rs`):
  `fork_conversation` clones both `base` and `init` from the source
  conversation.
- **User-facing commands**: `conversation edit -b` and `conversation path
  --base-config` continue to point at `base_config.json`.

Tests:

- New conversations write `base_config.json` in the `{ base, init }` format, and
  `from_parts()` / `to_parts()` round-trip a non-empty `init` list.
- Folding `base` + `init` produces the exact same `AppConfig` as resolving the
  entire creation invocation at once, including when a user-workspace override
  changes a model alias referenced by the workspace's `assistant.model.id`.
- Lower-precedence sources do not override workspace values during `init` replay
  (for example, `conversation.tools.'*'.run = "unattended"` in user-global
  config remains `"ask"` when set to `"ask"` in workspace config).
- Legacy flat `base_config.json` files load with `init: []`, are rewritten to `{
  base, init: [] }` on first persist, and are copied verbatim on subsequent
  persists.
- Changing a `#[setting(default)]` value does not change how an existing
  conversation resolves, both for normal conversations and for conversations
  created with a reset directive.
- Forking a conversation preserves both `base` and `init`.
- `conversation path --base-config` returns the `base_config.json` path.

Depends on Phase 2 and Phase 2b, and can be merged independently.

### Phase 4: `-C` flag and claim-history-driven revert

Wire `-C` / `--no-cfg` and the revert algorithms:

- **CLI wiring**: add `CfgDirective` (`Apply` and `Revert` around
  `KeyValueOrPath`), wire `-C` / `--no-cfg` in clap (rejecting bare `--no-cfg`),
  and merge `-c` and `-C` in command-line order via `ArgMatches::indices_of(..)`
  in `clap::FromArgMatches`.
- **Pipeline integration**: skip `CfgDirective::Revert` in phase 1
  (`partial_without_conversation`), and process both `Apply` and `Revert` in
  phase 2 starting from the folded `init` + `events` claims state seeded with
  environment unclaims.
- **Revert algorithms**:
  - **File-based `-C`** (identity-scoped): resolve candidate paths across roots,
    collect `hash(id)` and path hashes into `target_set`, select fields in
    `claims_state` whose claims intersect `target_set`, and walk backward to the
    first non-target claim or `base`.
  - **Key-value `-C`** (value-scoped): compare the field's current resolved
    value with the target value, skip with a diagnostic on mismatch, and walk
    backward to the first state with a different value or `base`.
  - **JSON-object `-C`**: expand via `try_merge_object` into per-leaf key-value
    reverts.
  - Emit diagnostics when a `-C` target matches nothing (see [Missing
    claims](#missing-claims-ux-and-diagnostics)).

Integration tests covering the worked examples:

- Basic cross-invocation revert
- Overlapping sources with identical values
- Shortcut flag override
- Key-value revert (matching and non-matching values, and undoing a file-set
  value)
- JSON-object revert equivalence with key-value revert
- Repeated claimant across invocations (`A -> B -> A`) and within one invocation
- Source file edited between invocations
- Diagnostics for unmatched `-C` targets

Depends on Phase 3.

## References

- [RFD 008]: Ordered Tool Directives, defining left-to-right processing for
  interleaved CLI flags.
- [RFD 035]: Multi-Root Config Load Path Resolution, defining the three-root
  search for `--cfg` paths reused by `-C`.
- **Conversation-ID inheritance** (`--cfg=jp-c<id>` and `--fork` implicit
  config), defined in a future RFD; inherited conversation partials claim every
  field they set under `hash(conversation_id)`.
- [RFD 038]: Config Reset Keywords, defining `--cfg=WORKSPACE` to opt a pinned
  conversation into current workspace config.
- [RFD 054]: Split Conversation Config and Events, separating `base_config.json`
  from `events.json`.
- [RFD 060]: Config Explain, which displays per-field provenance.
- `crates/jp_conversation/src/stream.rs`: `ConfigDelta` and
  `ConversationStream`.
- `crates/jp_cli/src/config_pipeline.rs`: `ConfigPipeline` and `apply_cfg_args`.
- `crates/jp_cli/src/lib.rs`: `KeyValueOrPath` and `-c` / `--cfg` parsing.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 035]: 035-multi-root-config-load-path-resolution.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 060]: 060-config-explain.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 079]: 079-config-sources-and-load-order.md
