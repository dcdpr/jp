# RFD 070: Negative Config Deltas

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-03

## Summary

This RFD introduces `-C` / `--no-cfg` as the negative counterpart of `-c` /
`--cfg`. A negative config argument accepts the same inputs as `-c` and reverts
the matching config's influence on the conversation. To support precise
per-source revert, each `ConfigDelta` gains a **claims map** that records which
config source last set each field. File-based `-C` uses claims for
provenance-based revert; key-value `-C` uses value comparison.

## Motivation

Today a user can layer config files onto a conversation:

```sh
jp query -n -c dev       # new conversation with dev overrides
jp query -c architect    # add architect overrides on top
```

There is no way to *remove* a previously applied config's influence. If the user
wants to stop using `dev` without starting a fresh conversation, they must
manually identify every field `dev` set and override each one with `--cfg
key=value`. This is tedious and error-prone.

The expected workflow is:

```sh
jp query -C dev          # "undo" dev's overrides
```

This should revert fields that `dev` introduced, but leave untouched any field
that was subsequently claimed by another config source (e.g. `architect`). If
both `dev` and `architect` set `tools = [read_file]`, reverting `dev` should not
disable the tool — `architect` still wants it.

If we do nothing, users must either track config state manually, start new
conversations when they want to change config profiles, or rely on `--cfg NONE`
([RFD 038]) which resets *everything* rather than selectively reverting one
source.

It should be noted that `-C` is one tool among several for managing config
state. `--cfg NONE` and `--cfg WORKSPACE` ([RFD 038]) provide clean-slate
alternatives when precise per-source revert is not needed. `-C` is the precision
tool for "undo this specific source."

## Design

### CLI surface

Add `-C` / `--no-cfg` as a global flag. It accepts exactly the same input syntax
as `-c` / `--cfg` — config file paths and `key=value` assignments:

```sh
jp query -C dev                                    # revert a file
jp query -C dev -c architect                       # revert dev, apply architect
jp query -C dev -C debug                           # revert both files
jp query -C assistant.name=JP                      # revert a single field
jp query -C dev -C assistant.model.id=anthropic    # mix file and key-value
```

Anything that `-c` can convert into a `PartialAppConfig`, `-C` can use as a
revert mask.

The two forms of `-C` use different revert mechanisms, though this is
transparent to the user:

- **File-based** (`-C dev`): uses **claims** (provenance tracking) to identify
  which fields the source owns, reverting only those. Precise even when multiple
  sources set the same field to the same value.
- **Key-value** (`-C foo=BAR`): uses **value comparison**. If the current value
  matches, revert it. No claims needed for single-field operations.

### Processing model

Negative args are processed left-to-right together with positive args, following
the ordered-directive model from [RFD 008]. A `-C` at any position in the
`--cfg`/`--no-cfg` sequence operates on whatever the accumulated state is at
that point.

```sh
# Left-to-right: apply dev, then revert dev (net effect: no dev)
jp query -c dev -C dev

# Left-to-right: revert dev first (no-op if not claimed), then apply architect
jp query -C dev -c architect
```

### Data model changes

#### `CfgDirective` wrapper

`KeyValueOrPath` is unchanged. A new wrapper enum captures whether a config arg
is additive or subtractive:

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
`CfgDirective::Revert`. Both share the same `KeyValueOrPath` resolution logic.
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
vs. file partials). `apply_cfg_args` matches the outer enum for polarity and the
inner for resolution type:

- `Apply(*)` — merge as today, plus record claims for `Partials`
- `Revert(Partials(...))` — file-based, use claims
- `Revert(KeyValue(...))` — key-value, use value comparison

#### Claims on `ConfigDelta`

Each `ConfigDelta` gains a claims map recording which config source last set
each field during that invocation:

```rust
pub struct ConfigDelta {
    pub timestamp: DateTime<Utc>,
    pub delta: Box<PartialAppConfig>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unsets: Vec<String>,

    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub claims: HashMap<String, Option<String>>,
}
```

- **`delta`**: the config diff, same as today.
- **`unsets`**: dotted field paths to reset to `None` after merging `delta`.
  Only populated by revert deltas, and only for optional fields whose target
  value is `None`. Normal deltas leave this empty. Applied in `stream.config()`
  **inside the delta replay loop**, immediately after merging each delta's
  partial. This ensures a later delta can re-set a field that an earlier delta
  unset. Implementation: add an `unset(path: &str)` method to `PartialAppConfig`
  (and nested partial types) that mirrors the existing `AssignKeyValue` dispatch
  but sets the target field to `None` instead of assigning a value. For
  vec-element unsets (see [Claim granularity](#claim-granularity)), a
  `remove_element(path: &str, element_json: &str)` method filters the target
  array. Both methods operate at the Rust type level, avoiding JSON
  serialization round-trips that would be fragile across custom serde
  implementations (e.g. `MergeableVec`, `MergedString`).
- **`claims`**: field path → `Some("HASH:LABEL")` (source that claimed this
  field) or `None` (explicitly unclaimed, see [Shortcut
  flags](#shortcut-flags)). Matching uses the `HASH` prefix; the `LABEL` is for
  display only.

#### Source identity

Each claim source is stored as a `HASH:LABEL` string. The hash (e.g. SHA-256) is
always present for identity matching. The label provides human-readable context
for provenance display (e.g. [RFD 060]) but varies by source location to avoid
leaking user-specific paths into shared workspace storage:

| Source type                     | Label                   | Example                            |
|---------------------------------|-------------------------|------------------------------------|
| File with `id` field            | The `id` value          | `a1b2c3:dev-persona`               |
| Workspace file (no `id`)        | Workspace-relative path | `d4e5f6:.jp/config/skill/dev.toml` |
| User-workspace file             | `<user-workspace>`      | `a1b2c3:<user-workspace>`          |
| User-local file                 | `<user-local>`          | `d4e5f6:<user-local>`              |
| Structured object with `id`     | The `id` value          | `f7a8b9:quick-model`               |
| Conversation ID                 | The conversation ID     | `c0d1e2:jp-c17528832001`           |
| Keyword (`NONE`, `WORKSPACE`)   | The keyword             | `000000:NONE`                      |
| Key-value assignment            | —                       | No claims; value comparison only   |
| Shortcut flag (`--model`, etc.) | —                       | Explicit unclaim (`None` entry)    |

The hash is computed from the source's **identity string**: the `id` field value
if present, or the resolved file path otherwise. For workspace files without
`id`, the workspace-relative path is hashed. For user-workspace and user-local
files without `id`, the absolute resolved path is hashed. Renaming a file
without an `id` field changes its hash and breaks claim matching for that source
— this is a known limitation that the `id` field exists to solve. When stable
cross-rename identity is needed, config files should declare an `id`.

Conversation streams are part of the workspace and typically shared via VCS.
Workspace-relative paths are safe to store verbatim. User-workspace and
user-local paths are redacted to placeholders since they may reveal personal
directory structure. The hash is always available as a fallback — `config
explain` ([RFD 060]) can attempt to resolve a placeholder by hashing all known
config files and matching against the stored hash.

#### Stable identity via `id`

Config files can declare an optional `id` field for stable identity:

```toml
# .jp/config/skill/dev.toml
id = "dev-persona"

[assistant]
name = "DevBot"
```

When present, the `id` is used instead of the file path for claim matching. This
survives file renames: if `dev.toml` is renamed to `developer.toml` but keeps
`id = "dev-cfg"`, `-C developer` still matches claims from the old file.

Two files with the same `id` are treated as the same config identity. This is
intentional — a team-shared config and a personal override with the same `id`
can replace each other without breaking claims.

The `id` field is added to `PartialAppConfig` as an optional field, similar to
the existing `inherit` field. It is read during `--cfg` resolution and stripped
(set to `None`) before merging, so it does not appear in the resolved
`AppConfig` or in persisted config deltas.

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
            source = file.source_id()  // "HASH:LABEL"
            for path in set_field_paths(&file.partial):
                claims[path] = Some(source)

    if Apply(kv):
        partial = assign(partial, kv)
        // no claim for key-value
```

A single `-c dev` can resolve to multiple files across config roots ([RFD 035]):
user-global, workspace, and user-workspace. Each file gets its own source
identity and claims. Files are merged in precedence order (user-global <
workspace < user-workspace), so if two files set the same field, the
higher-precedence file's claim overwrites the lower one.

If multiple files share the same `id` value, they hash to the same source
identity. Claims from all of them are attributed to that single identity. This
is intentional — all instances represent "the same config source" regardless of
which root they came from.

Within a single invocation, later `-c` args overwrite earlier claims for the
same field. This matches left-to-right merge semantics.

#### Shortcut flags

CLI shortcut flags (`--model`, `--tools`, `--reasoning`, etc.) are applied after
`-c` args by `apply_cli_config`. For any field a shortcut flag touches, the
corresponding claim is explicitly set to `None` (unclaimed):

```rust
// In apply_cli_config, after setting the model:
claims.insert("assistant.model.id".into(), None);
```

This prevents `-C dev` from reverting a field that `--model foo` deliberately
set. The `None` entry acts as a "stop walking" signal — when searching for a
field's claimant, an explicit `None` means "no source owns this field."

#### Persisting claims

The claims map built during the invocation is stored on the `ConfigDelta` event
alongside the diff. A `ConfigDelta` is emitted whenever the diff OR the claims
map is non-empty. This is a change from the current behavior, which skips empty
diffs — a claims-only delta (e.g. when `-c architect` sets a field to the same
value it already has) must still be stored to update provenance.

Old conversation streams without claims deserialize with an empty map
(`#[serde(default)]`), and `-C` falls back to value comparison for those fields.

### Revert algorithm

#### File-based `-C` (provenance)

When `-C dev` is encountered:

1. **Resolve** `dev` to one or more files across config roots ([RFD 035]),
   read each file's `id` (if any), compute source hashes.
2. **Load** each file into a `PartialAppConfig` to get the union of fields
   across all resolved files.
3. **Find claimants**: for each field in the partial, walk the conversation's
   `ConfigDelta` events backwards (most recent first). Find the first delta
   whose claims map contains an entry for that field:
   - `Some(hash)` where hash == dev's hash → **dev owns it**, mark for revert.
   - `Some(hash)` where hash != dev's hash → **another source owns it**, skip.
   - `None` → **explicitly unclaimed** (shortcut flag), skip.
   - Field not in any delta's claims → **no provenance data**, fall back to
     value comparison.
4. **Compute revert values**: for each field marked for revert, walk deltas
   backwards past **all** claims by the same source hash until finding a claim
   by a **different** source or reaching the base config. Use the config value
   at that point. If the target value is `None` (an optional field that was
   unset before the source claimed it), add the field path to the `unsets` list
   instead of the `delta` partial — schematic's merge cannot express `Some →
   None` transitions. This ensures that re-applying a source (e.g. `-c dev`
   after editing `dev.toml`) does not leave stale values from earlier
   applications when reverted.
5. **Emit** the reverted fields as part of the invocation's normal config
   pipeline. The final diff against the conversation's stored config produces a
   new `ConfigDelta` with updated claims (dev's entries removed).

#### Key-value `-C` (value comparison)

When `-C foo=BAR` is encountered:

1. Parse into a single-field `PartialAppConfig`.
2. If the current config's value for `foo` equals `BAR`, revert it.
3. Revert value: walk the delta history backwards to find the most recent
   different value. If none found, use the base config's value.

This is simpler than file-based revert and sufficient for single-field
operations where the user specifies both the field and the expected value.

### Conversation creation change

Currently, new conversations set `base_config` to the fully resolved `AppConfig`
(including environment variables, `-c` args, and CLI flags from the creation
invocation). This bakes all override values into the base with no `ConfigDelta`
stored and no claims recorded. A later `-C` has no claims to match against.

This RFD splits the initial conversation state across two files:

- **`base_config.json`** stores the pure workspace config: config files merged
  via inheritance, without environment variables, `-c` args, or CLI flags. This
  is a deterministic snapshot of the workspace's configuration state.

- **`init_config.json`** stores a `ConfigDelta` with claims, representing the
  difference between the workspace base and the fully resolved config used for
  the first turn. This captures everything layered on at conversation creation:
  environment variables, `-c` args, and CLI shortcut flags (`--model`,
  `--reasoning`, `--tool`, etc.).

```txt
Before:  base_config = files + env + (-c args + CLI flags)  → no delta, no claims
After:   base_config = files only (workspace config)
         init_config = delta(base → files + env + -c + CLI), with claims
```

This ensures every override — including the first invocation — produces a
`ConfigDelta` with claims that `-C` can later match against.

Separating env vars from the workspace base is deliberate. An env var like
`JP_CFG_ASSISTANT_MODEL_PARAMETERS_REASONING=high` is user intent for this
session, not a workspace property. If env overrides landed in `base_config.json`
while `--model` overrides landed in `init_config.json`, the split would be
arbitrary — both are per-invocation inputs. Storing them together in
`init_config.json` keeps the boundary clean: `base_config.json` is workspace
state, `init_config.json` is invocation state.

This preserves the readability win from [RFD 054]: `base_config.json` remains a
clean, inspectable workspace snapshot. `init_config.json` holds the initial
overrides with their claims. `events.json` contains only subsequent deltas and
conversation events — no leading config blob.

The `config()` method on `ConversationStream` folds `init_config` after
`base_config` and before `events`. `from_stored()` ([RFD 054]) gains a third
parameter for the initial delta.

### Parsing

`-C` reuses the same `KeyValueOrPath::from_str` parser as `-c`. The clap
definition wraps parsed values in `CfgDirective::Revert`:

```rust
#[arg(
    short = 'C',
    long = "no-cfg",
    global = true,
    action = ArgAction::Append,
    value_name = "KEY=VALUE",
    value_parser = KeyValueOrPath::from_str.map(CfgDirective::Revert),
)]
no_config: Vec<CfgDirective>,
```

Positive (`-c`) and negative (`-C`) args are merged into a single
`Vec<CfgDirective>` preserving command-line order, following the interleaving
pattern from [RFD 008].

A bare `--no-cfg` without a value reverts all claimed config overrides back to
the conversation's base config. This is useful as a "clean slate" before
layering new sources:

```sh
jp query -C -c dev    # revert everything, then apply dev fresh
```

This supersedes [RFD 038]'s definition of `--no-cfg` as shorthand for
`--cfg NONE`. The two operations are distinct: bare `-C` reverts claimed
overrides (provenance-based undo), while `--cfg NONE` resets all fields to
program defaults (value-based overwrite). Users who want the defaults reset use
`--cfg NONE` directly.

### Examples

#### Basic cross-invocation revert

```sh
jp query -n -c dev            # invocation 1
jp query -C dev -c committer  # invocation 2
```

**Invocation 1**: `base_config` = workspace files. Initial delta
(`init_config`) stored with diff (dev's field values plus any env var overrides)
and claims (`assistant.name → hash(dev)`,
`conversation.tools.read_file.enable → hash(dev)`, etc.).

**Invocation 2**: `-C dev` resolves dev, computes hash, walks claims. Dev owns
`assistant.name` → revert. Dev owns `tools` → revert. Then `-c committer` layers
on top. Final delta captures both the revert and committer's additions.

#### Overlapping sources — same field, same value

```sh
jp query -n -c dev      # invocation 1: tools=[read_file]
jp query -c architect   # invocation 2: tools=[read_file]
jp query -C dev         # invocation 3: revert dev
```

**Invocation 1**: delta claims `conversation.tools.read_file.enable →
hash(dev)`.

**Invocation 2**: architect also sets `tools.read_file.enable = true`. The
config value doesn't change, so the field is NOT in the delta's diff. But
architect's partial DOES set it, so the claims map records
`conversation.tools.read_file.enable → hash(architect)`, overwriting dev's
claim.

**Invocation 3**: `-C dev` checks: who owns
`conversation.tools.read_file.enable`? Most recent claim is `hash(architect)`
(from invocation 2). Dev is not the owner. **Skip.** Tools remain enabled.

#### Shortcut flag override

```sh
jp query -c dev           # invocation 1: model set by dev
jp query --model gpt-4o   # invocation 2: model overridden
jp query -C dev           # invocation 3: revert dev
```

**Invocation 1**: delta claims `assistant.model.id → hash(dev)`.

**Invocation 2**: `--model gpt-4o` is a shortcut flag. Delta diff has the new
model value. Claims map has `assistant.model.id → None` (explicitly unclaimed).

**Invocation 3**: `-C dev` checks: who owns `assistant.model.id`? Most recent
claim entry is `None` (invocation 2). **Skip.** The explicit `--model` override
is preserved.

#### Key-value revert

```sh
jp query -c assistant.name=DevBot   # invocation 1
jp query -C assistant.name=DevBot   # invocation 2
```

**Invocation 1**: key-value, no claims. Delta diff has `assistant.name = DevBot`.

**Invocation 2**: `-C assistant.name=DevBot` is key-value, uses value
comparison. Current value is `DevBot`, matches. Walk delta history backwards for
the previous different value. Revert to that value.

## Drawbacks

**Claims add storage overhead.** Each `ConfigDelta` gains a `HashMap` of field
paths to source hashes. For a typical persona file setting 10-20 fields, this is
a few hundred bytes per delta. Negligible in practice.

**Conversation creation change.** The conversation directory gains a fourth file
(`init_config.json`). `base_config.json` no longer contains the full resolved
config — it holds only the workspace config snapshot. Code that reads
`base_config` expecting the fully resolved config needs adjustment. The
steady-state persist cost is unchanged: `base_config.json` and
`init_config.json` are written once at creation, subsequent persists write only
`events.json` and `metadata.json`.

**Implementation cost.** The claims system touches the config pipeline,
`ConfigDelta` serialization, conversation creation, and shortcut flag
processing. Each piece is small but the surface area is broad. Notably,
`apply_cli_config` (the `IntoPartialAppConfig` trait) currently has no access to
the claims map — its signature must change to accept and mutate claims so that
shortcut flags can record explicit unclaims.

## Alternatives

### Snapshot stack (no provenance)

Replace the single-accumulator merge with a stack of intermediate snapshots.
`-C` walks the stack backwards comparing values to find the revert target.
Simpler to implement (no claims, no serialization changes), but fundamentally
limited: when two sources set the same field to the same value, value comparison
cannot distinguish them. The tools example (both `dev` and `architect` enabling
`read_file`) would incorrectly disable the tool when reverting dev. Rejected
because the common case of overlapping tool configurations makes this a real
problem, not a theoretical edge case.

### Per-flag ConfigDelta storage

Store one `ConfigDelta` per `-c` flag instead of one per invocation. Provides
finer-grained history but does not solve the core problem: if two `-c` flags in
the same invocation set the same field to the same value, the delta diffs are
identical. Only provenance tracking distinguishes them. Rejected as insufficient
on its own, though it could complement claims for within- invocation revert
precision.

### Full provenance tracking (tagged values)

Replace `Option<T>` fields in `PartialAppConfig` with `Tagged<T>` that carries a
source identifier. The most architecturally complete solution, but requires
changing the representation of every config field. Over-engineered for the
immediate use case. The claims map achieves the same result for revert purposes
without touching the config type system.

## Non-Goals

- **Direct stream editing.** `-C` does not remove or modify existing
  `ConfigDelta` events in the conversation stream. It influences the *next*
  delta by changing what the pipeline produces. The stream remains append-only.
- **Provenance display.** Showing which source contributed which field is useful
  but orthogonal. See [RFD 060].

## Risks and Open Questions

### Dual revert semantics

File-based `-C` uses provenance, key-value `-C` uses value comparison. These can
produce different results for the same field: if `dev` and `architect` both set
`assistant.name = DevBot`, then `-C dev` skips the field (architect owns it)
while `-C assistant.name=DevBot` reverts it. This is intentional — file-based
reverts guard against breaking overlapping configs (the values inside a file are
opaque to the user), while key-value reverts honor an explicit user request for
a specific field. It may surprise users who expect uniform behavior, but the
alternative (silently ignoring an explicit key-value revert due to an internal
claims system) would be worse.

### Claim granularity

Claims are recorded at **leaf level** — the finest granularity available. When
dev sets `tools.read_file.enable = true` and architect sets `tools.read_file.run
= "ask"`, the claims are:

```txt
conversation.tools.read_file.enable → hash(dev)
conversation.tools.read_file.run → hash(architect)
```

`-C dev` reverts `enable` only, leaving architect's `run` setting untouched.
This is more precise than entry-level claims (`conversation.tools.read_file`),
which would revert the entire tool config including fields dev never set.

Map-level claims (`conversation.tools → hash(dev)`) would be broken entirely:
architect enabling `write_file` would overwrite dev's claim on the whole map,
and `-C dev` would skip reverting `read_file` because architect "owns" tools.

Leaf-level paths are computed by serializing the source's `PartialAppConfig` to
JSON and walking the tree, collecting all non-null leaf paths:

```rust
fn set_field_paths(partial: &PartialAppConfig) -> Vec<String> {
    let value = serde_json::to_value(partial).unwrap_or_default();
    let mut paths = Vec::new();
    collect_paths(&value, String::new(), &mut paths);
    paths
}

fn collect_paths(value: &Value, prefix: String, out: &mut Vec<String>) {
    let Value::Object(map) = value else { return };
    for (key, val) in map {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match val {
            Value::Null => {}                                  // unset
            Value::Object(_) => collect_paths(val, path, out), // recurse
            _ => out.push(path),                               // leaf
        }
    }
}
```

This requires no new traits or manual implementations — it works generically
over any `PartialAppConfig` via serde. The serialization cost is negligible (one
small partial per invocation).

Vec fields (`attachments`, `config_load_paths`) cannot use positional indices
for claims (e.g. `conversation.attachments.0`) — if a later delta removes an
element, all subsequent indices shift and a revert would corrupt the array.

Instead, vec elements are claimed using their **serialized JSON value** as the
identity key. The `collect_paths` function handles arrays by serializing each
element:

```rust
Value::Array(arr) => {
    for val in arr {
        let val_str = serde_json::to_string(val).unwrap_or_default();
        out.push(format!("{prefix}[{val_str}]"));
    }
}
```

This produces claims like:

```txt
conversation.attachments["foo.rs"] → hash(dev)
conversation.attachments["bar.rs"] → hash(dev)
config_load_paths[".jp/agents"]    → hash(architect)
```

When reverting, claimed array elements go into `unsets` (since schematic's
`append_vec` merge can only add, not subtract). During the stream fold,
array-element unsets are applied by filtering the serialized array:

```rust
if let Some((array_path, element_json)) = unset_path.split_once("[")
    let element_str = &element_json[..element_json.len() - 1];
    if let Ok(element_val) = serde_json::from_str::<Value>(element_str) {
        if let Some(Value::Array(arr)) = get_mut_json_path(&mut root, array_path) {
            arr.retain(|x| x != &element_val);
        }
    }
}
```

The serialized value is the identity — no type-specific extraction needed.

This approach relies on deterministic JSON serialization. For scalar values and
simple structs, `serde_json::to_string` produces consistent output. For structs
with `HashMap` fields or other non-deterministic orderings, the serialized form
could vary between runs, breaking element identity. Current vec element types
(attachment paths, config load paths) are scalars or simple structs with
deterministic serialization. If struct-typed vec elements with non-deterministic
serialization are added in the future, they should use `BTreeMap` or implement a
canonical serialization form.

When computing array-element unsets, the revert algorithm must check whether the
element existed in the config state *before* the source first claimed it. If it
did (e.g. from `base_config` or an earlier delta), the source redundantly
declared it and reverting should not remove it. This uses the same claims
walk-back as scalar fields: walk past all claims by the same source, check if
the element is present in the config at that prior state.

### Interaction with RFD 038 keywords

`--cfg NONE` and `--cfg WORKSPACE` from [RFD 038] produce fully-populated
partials that overwrite everything. Like all `Apply` sources, they generate
claims (`hash("NONE")`, `hash("WORKSPACE")`). This keeps the claims pipeline
uniform — no special-case logic for keywords.

`-C NONE` is technically valid: it reverts all fields where NONE is still the
most recent claimant. In practice, any `-c` after NONE overwrites its claims for
the affected fields, so `-C NONE` only reverts fields that nothing else has
since claimed. This is a niche use case but falls out naturally from the uniform
design.

### Claims map size for large config files

A config file that sets many fields produces a large claims map. In practice,
persona files set 10-30 fields. If a config file sets hundreds of fields (e.g. a
full config dump), the claims map grows accordingly. This is bounded by the
total number of config fields in `AppConfig` and unlikely to be a performance
concern.

### Renamed or removed config fields

If a config field is renamed or removed across JP versions, old claims keyed on
the previous path become orphaned. These are harmlessly ignored — `-C` queries
against current field paths and skips entries it does not recognize.

### Backward compatibility

Old conversation streams have `ConfigDelta` events without claims. The
`#[serde(default)]` attribute initializes an empty `HashMap` for these. `-C`
falls back to value comparison when no claims exist for a field. This means `-C`
works (with best-effort precision) on conversations created before the claims
feature was added.

## Implementation Plan

### Phase 1: Claims and unsets on ConfigDelta

Add the `claims` and `unsets` fields to `ConfigDelta` in
`crates/jp_conversation/src/stream.rs`. Add the `id` field to
`PartialAppConfig` and strip it during config resolution. Implement source hash
computation (hash `id` if present, resolved path otherwise). Add the
`unset(path)` method to `PartialAppConfig` mirroring the `AssignKeyValue`
dispatch. Ensure backward-compatible deserialization with `#[serde(default)]`.

Tests: verify claims and unsets serialization round-trips, verify backward
compatibility with old `ConfigDelta` events (no claims field), verify `unset`
method for representative field types (scalar optional, nested struct, vec
element removal).

Can be merged independently (claims are populated but not yet used).

### Phase 2: Claims recording in the pipeline

Update `apply_cfg_args` and `apply_cli_config` to build the claims map during
config processing. `-c` file args record claims, `-c` key-value args do not,
shortcut flags explicitly unclaim. Thread the claims map through to
`ConfigDelta` creation.

Tests: verify claims are recorded correctly for each source type, verify
shortcut flags produce `None` entries, verify later `-c` args overwrite earlier
claims for the same field.

Depends on Phase 1. Can be merged independently.

### Phase 3: Conversation creation change

Split conversation creation to store `base_config.json` as the pure workspace
config (no env vars, no `-c` args, no CLI flags) and `init_config.json` as the
initial `ConfigDelta` with claims. Update `ConversationStream::from_stored()` to
accept the initial delta. Update `stream.config()` to fold `init_config` between
base and events. Add migration logic for existing conversations whose
`base_config` includes env vars and `-c` overrides.

Tests: verify new conversations produce the correct file split, verify
`from_stored()` with three inputs, verify migration of old-format conversations,
verify `config()` produces the same resolved result as before the split.

Depends on Phase 2. Can be merged independently.

### Phase 4: `-C` flag and revert logic

Add `CfgDirective` wrapper enum. Wire `-C`/`--no-cfg` in clap. Implement
file-based revert (claims walk-back) and key-value revert (value comparison).
Implement bare `-C` (revert all claims). Add integration tests covering all
worked examples from this RFD, including cross-invocation revert, overlapping
sources, shortcut flag override, key-value revert, and bare `-C` reset.

Depends on Phase 3.

## References

- [RFD 008]: Ordered Tool Directives — establishes left-to-right processing for
  interleaved CLI flags.
- [RFD 035]: Multi-Root Config Load Path Resolution — defines the three-root
  search for `--cfg` paths, which `-C` reuses.
- [RFD 038]: Config Inheritance for Conversations — defines `NONE` and
  `WORKSPACE` keywords for `--cfg`, which compose with `-C`.
- [RFD 054]: Split Conversation Config and Events — separates `base_config` into
  its own file, which this RFD keeps free of `-c` overrides.
- [RFD 060]: Config Explain — future RFD that may benefit from provenance data.
- `crates/jp_conversation/src/stream.rs` — `ConfigDelta` struct and conversation
  stream.
- `crates/jp_cli/src/config_pipeline.rs` — `ConfigPipeline` and
  `apply_cfg_args`.
- `crates/jp_cli/src/lib.rs` — `KeyValueOrPath` enum and `-c`/`--cfg` parsing.

[RFD 008]: 008-ordered-tool-directives.md
[RFD 035]: 035-multi-root-config-load-path-resolution.md
[RFD 038]: 038-config-inheritance-for-conversations.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 060]: 060-config-explain.md
