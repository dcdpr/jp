# RFD D35: Extends Overrides and Loader Namespace

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-09
- **Requires**: [RFD 035](../035-multi-root-config-load-path-resolution.md), [RFD 038](../038-config-reset-keywords.md), [RFD 079](../079-config-sources-and-load-order.md)

## Summary

This RFD adds `loader.overrides.extends`, an entry-scoped way to exclude a
config source while resolving the `extends` tree for a specific `--cfg` entry.
It also moves existing load-time controls into a `loader` namespace: `extends`
becomes `loader.extends`, `config_load_paths` becomes `loader.search_paths`, and
`inherit` becomes `loader.inherit`.

## Motivation

JP lets users compose named config files with `extends`. A workspace can define a
shared `dev` entry that extends other config files:

```toml
# .jp/config/entries/dev.toml
[loader]
extends = [
    "../fragments/web-access.toml",
    "../fragments/local-context.toml",
]
```

A user may want to keep using `jp q -c dev` but remove the web-related fragment
from that entry in their private config. Higher-precedence config can override
final field values, but it cannot currently say "do not load this extended
source as part of this entry." That matters when the extended source contributes
multiple fields, such as tool configuration and prompt sections. Disabling one
field after the merge leaves the rest of the source's contribution behind.

The desired behavior is source-level and entry-scoped:

```sh
jp q -c dev
```

filters the web-related source from the `dev` entry, while:

```sh
jp q -c dev -c fragments/web-access
```

still loads the web-related source explicitly as its own entry.

The design also has to survive indirection. If `dev.toml` later extends an
intermediate file, and that intermediate file extends `web-access.toml`, the
user still wants to filter `web-access.toml` only when the active entry is
`dev.toml`. Filtering the immediate edge would either miss the source or affect
other entries that share the intermediate file.

## Design

### Entry loading

This RFD uses **entry loading** to mean config loaded explicitly through `--cfg`.
[RFD 079] calls this deferred loading. An **entry** is one concrete config file
resolved from a `--cfg <name>` argument. Because multi-root resolution can find
multiple files for one argument, each resolved file is its own entry with its own
source identity and `extends` tree.

`loader.overrides.extends` applies only while resolving these explicit entries.
It does not affect implicit base config loading.

### User-facing configuration

Add entry-scoped `extends` override rules under `loader.overrides.extends`:

```toml
[[loader.overrides.extends]]
within = { root = "workspace", path = ".jp/config/entries/dev.toml" }
exclude = [
    ".jp/config/fragments/web-access.toml",
]
```

`within` identifies the concrete entry whose `extends` tree is patched.
`exclude` lists sources to skip anywhere inside that entry's tree.

The rule means:

> When resolving the `extends` tree for the workspace `dev.toml` entry, skip the
> workspace `web-access.toml` source anywhere inside that tree. Do not skip
> `web-access.toml` when it is loaded as its own entry or through another entry.

This is entry-scoped and transitive. A rule for `dev.toml` applies anywhere
inside `dev.toml`'s `extends` tree, including through intermediate config files.

### Source selectors

`within` uses a strict root-qualified source selector:

```toml
{ root = "workspace", path = ".jp/config/entries/dev.toml" }
```

`root` is one of the config roots used when resolving named `--cfg` files:

| Root | Path base |
|------|-----------|
| `user-global` | `<user-config-dir>/config/` |
| `workspace` | `<workspace-root>/` |
| `user-workspace` | `<user-workspace-dir>/config/` |

`path` is relative to that root. Absolute paths are rejected. Paths that escape
the root are rejected. Matching uses the normalized source identity after file
resolution.

`exclude` paths are root-relative paths resolved inside `within.root`:

```toml
exclude = [".jp/config/fragments/web-access.toml"]
```

The root is inferred from `within` because `loader.extends` is a same-root
composition mechanism. A config file in the workspace root can extend other
workspace-root files; a config file in the user-workspace root can extend other
user-workspace-root files.

### Exclude behavior

Given this rule:

```toml
[[loader.overrides.extends]]
within = { root = "workspace", path = ".jp/config/entries/dev.toml" }
exclude = [".jp/config/fragments/web-access.toml"]
```

JP behaves as follows:

| Invocation | Behavior |
|------------|----------|
| `jp q -c dev` | `web-access.toml` is excluded from the workspace `dev.toml` entry. |
| `jp q -c dev -c fragments/web-access` | `web-access.toml` is excluded from `dev`, then loaded as its own entry. |
| `jp q -c research` | `web-access.toml` loads if `research.toml` extends it. |
| `jp q -c dev -c research` | `web-access.toml` is excluded from `dev`, but can still load through `research`. |

The rule applies through indirection:

```toml
# .jp/config/entries/dev.toml
[loader]
extends = ["../bundles/standard.toml"]

# .jp/config/bundles/standard.toml
[loader]
extends = ["../fragments/web-access.toml", "../fragments/local-context.toml"]
```

When the active entry is `dev.toml`, `web-access.toml` is skipped even though
the direct edge is `standard.toml -> web-access.toml`. When the same
intermediate file is reached through a different entry, the rule does not fire.

### Where override rules come from

Override rules are read only from the invocation's base partial:

```text
implicit files + environment variables
```

This matches `loader.search_paths`: lookup controls are read once when the
config pipeline is constructed. Rules introduced by a `--cfg` file, conversation
config, or command-specific CLI shortcuts do not affect `--cfg` resolution in
the same invocation.

This avoids confusing left-to-right behavior such as:

```sh
jp q -c my-overrides -c dev
```

where `my-overrides` would affect only later `--cfg` entries.

### Loader model

`ConfigPipeline::new` extracts `loader.overrides.extends` rules from the base
partial and passes them into `resolve_cfg_args`. Resolved file arguments keep
source identity alongside the loaded partial:

```rust
struct ResolvedCfgEntry {
    identity: Option<ConfigSourceIdentity>,
    path: Utf8PathBuf,
    partial: PartialAppConfig,
}
```

Named `--cfg` entries receive an identity after `find_file_in_load_path`
resolves them. Explicit filesystem paths receive an identity only when they
normalize under a known config root. Paths outside known roots still load, but
have no active entry identity for override matching.

The recursive loader gains an optional override context containing the active
entry identity, the base-layer rules, and the known roots. The implicit
config-loading path passes no context and keeps today's behavior. At each
extended source candidate, JP normalizes the candidate source, skips candidates
that match the active entry's `exclude` list, and loads the remaining sources
with normal `before` / `after` strategy handling.

Glob-expanded files are normal candidates. Each concrete file produced by a glob
is normalized and checked independently. `overrides` does not support
glob-pattern selectors; users name concrete sources.

### Loader namespace cleanup

Because this RFD introduces the `loader` namespace for
`loader.overrides.extends`, it also moves the existing root-level loader fields
into that namespace:

```toml
[loader]
inherit = true
search_paths = [".jp/config", ".jp/config/entries"]
extends = [
    "config.d/**/*",
    { path = "./foo/baz.toml", strategy = "after" },
]
```

The field mapping is:

| Current field | New field | Meaning |
|---------------|-----------|---------|
| `extends` | `loader.extends` | Files this config source extends. |
| `config_load_paths` | `loader.search_paths` | Directories searched by named `--cfg` arguments. |
| `inherit` | `loader.inherit` | Whether implicit file loading continues after this source. |

[RFD 038] defines `loader.reset`, which uses the same namespace but is not
designed here.

### Persistence

`loader` fields are load-time metadata. Config files may contain them, but they
must not be persisted into conversation deltas or `base_config.json`.
Conversation-targeted `jp config set` for this namespace should fail with a
clear error rather than storing values that cannot affect future loader
construction.

## Drawbacks

Entry-scoped overrides add action at a distance. A base-layer config can change
how another entry's `extends` tree resolves. The `within` selector and
structured diagnostics are required to make this understandable.

This is also a breaking config-schema change. Users must move existing
root-level loader fields into `[loader]`.

## Alternatives

### Keep root-level loader fields

JP could keep `extends`, `config_load_paths`, and `inherit` at the document root
and add only `config.extends.exclude`. This minimizes migration work, but keeps
the existing schema confusion and gives the new override behavior an awkward
home.

### Target-only source exclusion

A broader target-only shape was considered:

```toml
[config.sources]
exclude = [".jp/config/fragments/web-access.toml"]
```

This is too broad. It would suppress `web-access.toml` from `dev`, but also from
any other entry or intermediate file that extends it. It also risks blocking a
later explicit `-c fragments/web-access` load unless direct entries are
specially protected.

### Generic `loader.overrides.*`

A generic override system for every loader field was considered:

```toml
[loader.overrides]
# hypothetical future shape
```

Rejected for this RFD. `loader.extends`, `loader.search_paths`, and
`loader.inherit` run in different phases. `extends` is a graph and supports
entry-scoped graph patching cleanly. `search_paths` affects whether named
entries can be found before those entries exist, and `inherit` controls the
implicit file-source cascade. They need separate designs if real use cases
appear.

### Include overrides

`loader.overrides.extends` could also support injecting sources into another
entry's `extends` tree:

```toml
[[loader.overrides.extends]]
within = { root = "workspace", path = ".jp/config/entries/dev.toml" }
include = [
    { path = ".jp/config/fragments/local-dev.toml", strategy = "after" },
]
```

This is useful, but it needs more design work around ordering, placement, and
cross-root behavior. This RFD reserves the `include` field for a future proposal
but does not define it.

### JSON Patch

A generic JSON Patch system was considered. JSON Patch is useful for document
surgery, but it operates on serialized config shape rather than loader source
identity. JP already has typed layered config for ordinary application-setting
changes. The missing behavior here is specifically source-level `extends` tree
filtering, so a domain-specific override is clearer.

### Edge-scoped exclusion

An edge-scoped rule would name the immediate parent and child. This works while
entry files directly extend every fragment. It breaks when an intermediate file
is introduced between the entry and the fragment: `dev.toml` no longer has a
direct edge to `web-access.toml`, while filtering the intermediate file's edge
would affect all other entries that use the same file.

### Field-level removal

Removing prompt sections or tools by identity is useful in its own right, but it
requires the user to know every field contributed by the source. If
`fragments/web-access.toml` later adds more config, the removal silently becomes
incomplete.

## Non-Goals

- A generic override mechanism for every loader field.
- Include overrides for `loader.extends`.
- Wildcards, omitted roots, or path-only selectors for `within`.
- Cross-root excludes.
- A global source denylist.
- Exact edge-scoped filtering.
- Field-level tombstones or vector element removal.
- `loader.reset`; it is defined by [RFD 038].

## Risks and Open Questions

**Diagnostics.** Users need visibility into why a source was skipped. Future `jp
config explain` work could surface extends overrides in the resolved config
explanation.

**Rule merge behavior.** Extends override rules should accumulate across base
layers with order-preserving deduplication. There is no RFD mechanism to remove
an inherited override rule.

**RFD 070 attribution.** This RFD assumes RFD 070's current model where extended
files are attributed to the parent `--cfg` source for claim history. If RFD 070
later records extended-file claims separately, the `-C` alternative should be
revisited, but this RFD remains loader-time graph patching rather than a
conversation-history revert.

## Implementation Plan

1. Add `loader.overrides.extends`, source selectors, and config-root identifiers
   to `jp_config`.
2. Extract `loader.overrides.extends` from the base partial in
   `ConfigPipeline::new` and pass the rules into `resolve_cfg_args`.
3. Track source identities while resolving `--cfg` paths across config roots.
   Each resolved file becomes a `ResolvedCfgEntry` with its path, partial, and
   optional active entry identity.
4. Thread an optional override context through the recursive config loader. The
   implicit config-loading path passes no context; `--cfg` entry loading passes
   the active entry identity and rule set.
5. Implement exclude handling for normal extends, glob-expanded candidates, and
   both `before` and `after` strategies.
6. Move root-level `extends`, `config_load_paths`, and `inherit` into
   `loader.extends`, `loader.search_paths`, and `loader.inherit`.
7. Add migration diagnostics for old root-level fields. Because this RFD is a
   breaking reset, JP may reject old fields with clear replacement messages
   rather than accepting aliases indefinitely.
8. Keep all `loader` fields out of persisted conversation deltas and
   `base_config.json`. Reject conversation-targeted `jp config set` writes for
   `loader.*` with a clear error.
9. Add tests for direct exclusion, indirection, glob-expanded `loader.extends`,
   `strategy = "after"`, explicit later `-c fragments/web-access`, another entry
   extending the same source, multi-root `--cfg dev` where only one resolved
   entry is patched, strict root identity, explicit paths under known roots,
   explicit paths outside known roots, invalid selectors, unmatched selectors,
   and rule accumulation with order-preserving deduplication.
10. Update [RFD 079], `docs/configuration.md`, examples, and project config
    files to use the new loader namespace.

## References

- [RFD 035] — multi-root config load path resolution.
- [RFD 038] — config reset keywords and `loader.reset`.
- [RFD 070] — negative config deltas and `-C` claim attribution.
- [RFD 079] — config sources and load order.
- `crates/jp_cli/src/config_pipeline.rs` — `ConfigPipeline::new` and
  `resolve_cfg_args`.
- `crates/jp_config/src/util.rs` — recursive `extends` loading.

[RFD 035]: ../035-multi-root-config-load-path-resolution.md
[RFD 038]: ../038-config-reset-keywords.md
[RFD 070]: ../070-negative-config-deltas.md
[RFD 079]: ../079-config-sources-and-load-order.md
