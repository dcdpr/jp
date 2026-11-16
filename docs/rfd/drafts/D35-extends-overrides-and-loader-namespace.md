# RFD D35: Extends Overrides and Loader Namespace

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-09
- **Requires**: [RFD 079]
- **Extends**: [RFD 038]
- **Summary**: Adds entry-scoped `loader.overrides.extends` exclusions and
  consolidates `extends`, `search_paths`, and `inherit` under a `loader` config
  namespace.

## Summary

This RFD adds `loader.overrides.extends` to exclude specific config sources when
resolving the `extends` tree of a `--cfg` entry.
It also moves existing load-time controls into the `loader` namespace: `extends`
becomes `loader.extends`, `config_load_paths` becomes `loader.search_paths`, and
`inherit` becomes `loader.inherit`.

## Motivation

JP lets users compose named config files with `extends`.
A workspace can define a shared `dev` entry that extends other config files:

```toml
# .jp/config/entries/dev.toml
[loader]
extends = [
    "../fragments/web-access.toml",
    "../fragments/local-context.toml",
]
```

A user may want to run `jp q -c dev` while excluding `web-access.toml` from that
entry in their private config.
Higher-precedence config can override merged field values, but cannot skip an
extended source when loading an entry.
When an extended source sets multiple fields, such as tool configuration and
prompt sections, overriding individual fields after the merge leaves the rest of
the source's settings in place.

Exclusion must operate at the source level and scope to a specific entry.
Running:

```sh
jp q -c dev
```

skips `web-access.toml` while loading `dev`, whereas:

```sh
jp q -c dev -c fragments/web-access
```

still loads `web-access.toml` when requested as its own entry.

Exclusion must also hold across transitive `extends` chains.
If `dev.toml` extends an intermediate file that in turn extends
`web-access.toml`, `web-access.toml` should still be excluded when loading
`dev.toml`.
Filtering only direct edges would either miss `web-access.toml` once the
intermediate file is introduced or exclude it from every other entry that shares
the intermediate file.

## Design

### Entry loading

This RFD uses **entry loading** for config loaded explicitly via `--cfg` (called
deferred loading in [RFD 079]).
An **entry** is a single config file resolved from a `--cfg <name>` argument.
When multi-root resolution matches multiple files for one argument, each
resolved file is a separate entry with its own source identity and `extends`
tree.

`loader.overrides.extends` applies only during entry loading; it does not affect
implicit base config loading.

### Override rules

`loader.overrides.extends` defines entry-scoped exclusion rules:

```toml
[[loader.overrides.extends]]
within = { root = "workspace", path = ".jp/config/entries/dev.toml" }
exclude = [
    ".jp/config/fragments/web-access.toml",
]
```

`within` selects the concrete entry whose `extends` tree is patched, and
`exclude` lists the sources to skip anywhere in that tree.
Exclusion is entry-scoped and transitive: the rule above skips the workspace
`web-access.toml` anywhere in the workspace `dev.toml` entry's `extends` tree,
including through intermediate config files, without skipping `web-access.toml`
when it is loaded as its own entry or through another entry.

### Source selectors

`within` uses a strict root-qualified source selector:

```toml
{ root = "workspace", path = ".jp/config/entries/dev.toml" }
```

`root` is one of the config roots used when resolving named `--cfg` files:

| Root             | Path base                      |
| ---------------- | ------------------------------ |
| `user-global`    | `<user-config-dir>/config/`    |
| `workspace`      | `<workspace-root>/`            |
| `user-workspace` | `<user-workspace-dir>/config/` |

`path` is relative to `root`; absolute paths and paths that escape the root are
rejected.
Selectors match against the normalized source identity after file resolution.

`exclude` entries are paths relative to `within.root`:

```toml
exclude = [".jp/config/fragments/web-access.toml"]
```

`exclude` inherits `within.root` because `loader.extends` only composes files
within the same root: a workspace config file can only extend other
workspace-root files, and a user-workspace file can only extend other
user-workspace-root files.

### Exclude behavior

Given this rule:

```toml
[[loader.overrides.extends]]
within = { root = "workspace", path = ".jp/config/entries/dev.toml" }
exclude = [".jp/config/fragments/web-access.toml"]
```

JP behaves as follows:

| Invocation                            | Behavior                                                                         |
| ------------------------------------- | -------------------------------------------------------------------------------- |
| `jp q -c dev`                         | `web-access.toml` is excluded from the workspace `dev.toml` entry.               |
| `jp q -c dev -c fragments/web-access` | `web-access.toml` is excluded from `dev`, then loaded as its own entry.          |
| `jp q -c research`                    | `web-access.toml` loads if `research.toml` extends it.                           |
| `jp q -c dev -c research`             | `web-access.toml` is excluded from `dev`, but can still load through `research`. |

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
the direct edge is `standard.toml -> web-access.toml`.
When `standard.toml` is reached through a different entry, `web-access.toml`
loads normally.

### Rule sources

Override rules are read only from the invocation's base partial:

```text
implicit files + environment variables
```

Like `loader.search_paths`, override rules are read once when constructing the
config pipeline.
Rules in a `--cfg` file, conversation config, or command-specific CLI shortcuts
do not affect `--cfg` resolution in the same invocation.
This avoids order-dependent behavior across `--cfg` arguments, such as:

```sh
jp q -c my-overrides -c dev
```

where `my-overrides` would otherwise affect only subsequent `--cfg` entries.

### Loader model

`ConfigPipeline::new` extracts `loader.overrides.extends` rules from the base
partial and passes them into `resolve_cfg_args`.
Resolved file arguments keep their source identity alongside the loaded partial:

```rust
struct ResolvedCfgEntry {
    identity: Option<ConfigSourceIdentity>,
    path: Utf8PathBuf,
    partial: PartialAppConfig,
}
```

Named `--cfg` entries receive an identity after `find_file_in_load_path`
resolves them.
Explicit filesystem paths receive an identity only when they normalize under a
known config root; paths outside known roots still load, but carry no active
entry identity for override matching.

The recursive loader takes an optional override context containing the active
entry identity, the base-layer rules, and the known roots.
Implicit config loading passes no override context.
For each extended source candidate, JP normalizes the path, skips it if it
matches the active entry's `exclude` list, and otherwise loads it according to
its `before` or `after` strategy.

When an `extends` entry uses a glob, each matched file is normalized and checked
against `exclude` individually.
`loader.overrides.extends` selectors do not support globs and must name concrete
files.

### Loader namespace

Alongside `loader.overrides.extends`, this RFD moves the root-level loader
fields into the `loader` namespace:

```toml
[loader]
inherit = true
search_paths = [".jp/config", ".jp/config/entries"]
extends = [
    "config.d/**/*",
    { path = "./foo/baz.toml", strategy = "after" },
]
```

| Current field       | New field             | Meaning                                                    |
| ------------------- | --------------------- | ---------------------------------------------------------- |
| `extends`           | `loader.extends`      | Files this config source extends.                          |
| `config_load_paths` | `loader.search_paths` | Directories searched by named `--cfg` arguments.           |
| `inherit`           | `loader.inherit`      | Whether implicit file loading continues after this source. |

[RFD 038] defines `loader.reset` in the same namespace.

### Persistence

`loader` fields are load-time metadata used only when reading config files.
They must not be persisted into conversation deltas or `base_config.json`.
Conversation-targeted `jp config set` writes to `loader.*` fail with an error
rather than storing values that have no effect on subsequent loader
construction.

## Drawbacks

Entry-scoped overrides introduce action at a distance: a base-layer config file
can alter how a separate entry's `extends` tree resolves.
Explicit `within` selectors and structured diagnostics are needed to keep this
behavior traceable.

Moving `extends`, `config_load_paths`, and `inherit` under `[loader]` is a
breaking config-schema change that requires migrating existing config files.

## Alternatives

### Keep root-level loader fields

JP could keep `extends`, `config_load_paths`, and `inherit` at the document root
and add only `config.extends.exclude`.
This avoids migrating existing files, but leaves load-time directives mixed with
application settings at the root and splits `extends` configuration across two
namespaces.

### Target-only source exclusion

Excluding a source globally without a `within` selector:

```toml
[config.sources]
exclude = [".jp/config/fragments/web-access.toml"]
```

This is too broad: it suppresses `web-access.toml` not only from `dev`, but from
every other entry or intermediate file that extends it, and blocks an explicit
`-c fragments/web-access` load unless direct entries are special-cased.

### Generic `loader.overrides.*`

A single override mechanism covering every loader field:

```toml
[loader.overrides]
# hypothetical future shape
```

`loader.extends`, `loader.search_paths`, and `loader.inherit` run at different
stages of config loading.
`extends` forms a per-entry graph that supports entry-scoped graph patching,
whereas `search_paths` governs how named entries are located before those
entries exist and `inherit` controls the implicit file-source cascade.
A uniform override model does not fit all three.

### Include overrides

`loader.overrides.extends` could also inject sources into another entry's
`extends` tree:

```toml
[[loader.overrides.extends]]
within = { root = "workspace", path = ".jp/config/entries/dev.toml" }
include = [
    { path = ".jp/config/fragments/local-dev.toml", strategy = "after" },
]
```

Injecting sources requires additional design around ordering, placement, and
cross-root behavior.
This RFD reserves the `include` field for a future proposal without defining it
here.

### JSON Patch

JSON Patch operates on serialized config values rather than loader source
identity.
JP's layered config already handles value-level overrides; the missing
capability is skipping a source file in an `extends` tree before its fields are
merged, which a source-level exclusion rule expresses directly.

### Edge-scoped exclusion

An edge-scoped rule would match a direct `(parent, child)` pair in the `extends`
graph.
That breaks when an intermediate file is introduced between an entry and a
fragment: `dev.toml` no longer has a direct edge to `web-access.toml`, and
excluding the edge on the intermediate file would affect every other entry that
extends it.

### Field-level removal

Removing individual prompt sections or tools by identity requires the user to
know every field an extended source contributes.
If `fragments/web-access.toml` later adds another setting, field-level overrides
silently become incomplete.

## Non-Goals

- A generic override mechanism for every loader field.
- Include overrides for `loader.extends`.
- Wildcards, omitted roots, or path-only selectors for `within`.
- Cross-root excludes.
- A global source denylist.
- Exact edge-scoped filtering.
- Field-level tombstones or vector element removal.
- `loader.reset` (defined by [RFD 038]).

## Risks and Open Questions

**Diagnostics.** Skipping a file in an `extends` tree is invisible unless
reported.
Future `jp config explain` work should surface `loader.overrides.extends` rules
and the sources they skipped.

**Rule merge behavior.** `loader.overrides.extends` rules accumulate across base
layers with order-preserving deduplication.
This RFD does not provide a way for a higher-precedence base layer to remove an
inherited override rule.

**RFD 070 attribution.** [RFD 070] attributes fields from extended files to
their parent `--cfg` source in claim history, so `-C` cannot revert a single
extended file without reverting the parent entry.
Even if [RFD 070] later records claims per extended file, `-C` reverts claims in
conversation history whereas `loader.overrides.extends` patches the `extends`
graph at load time.

## Implementation Plan

1. Add `loader.overrides.extends`, source selectors, and config-root identifiers
   to `jp_config`.
2. Extract `loader.overrides.extends` from the base partial in
   `ConfigPipeline::new` and pass the rules into `resolve_cfg_args`.
3. Track source identities while resolving `--cfg` paths across config roots.
   Each resolved file becomes a `ResolvedCfgEntry` with its path, partial, and
   optional active entry identity.
4. Thread an optional override context through the recursive config loader.
   Implicit config loading passes no context; `--cfg` entry loading passes the
   active entry identity and rule set.
5. Filter excluded sources in the recursive loader across direct `extends`,
   glob-expanded candidates, and both `before` and `after` strategies.
6. Move root-level `extends`, `config_load_paths`, and `inherit` into
   `loader.extends`, `loader.search_paths`, and `loader.inherit`.
7. Add migration diagnostics that reject the old root-level `extends`,
   `config_load_paths`, and `inherit` fields with errors naming their `loader.*`
   replacements.
8. Keep all `loader` fields out of persisted conversation deltas and
   `base_config.json`, and reject conversation-targeted `jp config set` writes
   to `loader.*` with a clear error.
9. Add tests for direct exclusion, indirection, glob-expanded `loader.extends`,
   `strategy = "after"`, explicit later `-c fragments/web-access`, another entry
   extending the same source, multi-root `--cfg dev` where only one resolved
   entry is patched, strict root identity, explicit paths under known roots,
   explicit paths outside known roots, invalid selectors, unmatched selectors,
   and rule accumulation with order-preserving deduplication.
10. Update [RFD 079], `docs/configuration.md`, examples, and project config
    files to use the new loader namespace.

## References

- [RFD 035]: multi-root config load path resolution.
- [RFD 038]: config reset keywords and `loader.reset`.
- [RFD 070]: negative config deltas and `-C` claim attribution.
- [RFD 079]: config sources and load order.
- `crates/jp_cli/src/config_pipeline.rs`: `ConfigPipeline::new` and
  `resolve_cfg_args`.
- `crates/jp_config/src/util.rs`: recursive `extends` loading.

[RFD 035]: ../035-multi-root-config-load-path-resolution.md
[RFD 038]: ../038-config-reset-keywords.md
[RFD 070]: ../070-negative-config-deltas.md
[RFD 079]: ../079-config-sources-and-load-order.md
