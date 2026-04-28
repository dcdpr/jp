# RFD 035: Multi-Root Config Load Path Resolution

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08

## Summary

This RFD extends `--cfg` path resolution to search across all three config roots
(user-global, workspace, user-workspace), merging matches in precedence order.
Currently, `--cfg` paths are resolved exclusively against the workspace root,
meaning user-global and user-workspace config files are unreachable.

## Motivation

JP loads `config.toml` from three locations, in precedence order:

1. **User-global**: `$XDG_CONFIG_HOME/jp/config.toml` — private to the user,
   shared across all workspaces.
2. **Workspace**: `<workspace_root>/.jp/config.toml` — shared with the team via
   version control.
3. **User-workspace**: `$XDG_DATA_HOME/jp/workspace/<id>/config.toml` — private
   to the user, scoped to a single workspace.

This layering lets teams define shared defaults while individual users override
them privately. The `--cfg` flag extends this by loading named config fragments
(like `skill/web` or `personas/dev`) from directories listed in
`config_load_paths`.

The problem: `--cfg` resolution only searches `config_load_paths` relative to
the workspace root. A user who places `skill/web.toml` in their user-global
config directory gets an error — the file is never found. There is no way to
define personal `--cfg`-loadable config fragments that apply across all
workspaces, or personal fragments scoped to a single workspace.

This breaks the layering model. A user who wants a personal `skill/web` override
must either modify the workspace config (polluting the shared config), or pass
the full path every time (`--cfg $XDG_CONFIG_HOME/jp/...`).

## Design

### Current behavior

`load_cli_cfg_args` in `crates/jp_cli/src/lib.rs` handles `--cfg` path
arguments. When the argument is not an existing file and not a key-value pair,
it iterates `config_load_paths`, resolving each entry against
`workspace.root()`:

```rust
let config_load_paths = workspace.iter().flat_map(|w| {
    partial.config_load_paths.iter().flatten().filter_map(|p| {
        Utf8PathBuf::try_from(p.to_path(w.root()))
        // ...
    })
});
```

It then calls `find_file_in_load_path` for each resolved path and stops at the
first match (`break`). Only the workspace root is ever searched.

### Proposed behavior

Resolve `config_load_paths` against three roots instead of one, searching each
in precedence order (lowest to highest). If a match is found in multiple roots,
all matches are loaded and merged, with later roots taking precedence.

The three roots, and how `config_load_paths` entries are resolved against them:

| Precedence  | Source         | Resolution root                            |
|-------------|----------------|--------------------------------------------|
| 1 (lowest)  | User-global    | `$XDG_CONFIG_HOME/jp/config/`              |
| 2           | Workspace      | `<workspace_root>/`                        |
| 3 (highest) | User-workspace | `$XDG_DATA_HOME/jp/workspace/<id>/config/` |

The user-global and user-workspace roots use a `config/` subdirectory as the
resolution base. This prevents `config_load_paths` entries from polluting the
top-level directory structure of those locations, which contain non-config
entries (`workspace/`, `storage` symlink, `conversations/`, etc.).

The workspace root does not need this sandboxing — `config_load_paths` entries
like `.jp/config` are already scoped by convention.

### Example

Given `config_load_paths = [".jp/config"]` and `--cfg skill/web`, the search
order is:

1. `$XDG_CONFIG_HOME/jp/config/.jp/config/skill/web.toml`
2. `<workspace_root>/.jp/config/skill/web.toml`
3. `$XDG_CONFIG_HOME/jp/workspace/<id>/config/.jp/config/skill/web.toml`

If files exist at positions 1 and 2, both are loaded. The workspace file (2) is
merged on top of the user-global file (1). If only one exists, it is loaded
as-is.

### Merge behavior

Within a single root, the existing first-match-wins behavior is preserved — if
`config_load_paths` contains multiple entries, only the first entry that
produces a match is used. Across roots, all matches are merged in precedence
order using the existing `load_partial` merge function.

### Code changes

The change is localized to the `KeyValueOrPath::Path` branch in
`load_cli_cfg_args` (`crates/jp_cli/src/lib.rs`). The function already receives
`workspace: Option<&Workspace>`, which provides access to both `root()` and
`user_storage_path()`. The user-global path is available via
`user_global_config_dir()`.

Sketch:

```rust
KeyValueOrPath::Path(path) => {
    let home = std::env::home_dir()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok());

    // Build search roots in precedence order (lowest first).
    let mut roots: Vec<Utf8PathBuf> = Vec::new();

    if let Some(global_dir) = user_global_config_dir(home.as_deref()) {
        roots.push(global_dir.join("config"));
    }
    if let Some(w) = workspace {
        roots.push(w.root().to_owned());
    }
    if let Some(user_ws_dir) = workspace.and_then(Workspace::user_storage_path) {
        roots.push(user_ws_dir.join("config"));
    }

    let mut matches: Vec<PartialAppConfig> = Vec::new();

    for root in &roots {
        for load_rel in partial.config_load_paths.iter().flatten() {
            let Ok(load_path) = Utf8PathBuf::try_from(load_rel.to_path(root))
            else {
                continue;
            };
            if let Some(file) = find_file_in_load_path(path, &load_path) {
                if let Some(p) = load_partial_at_path(file)? {
                    matches.push(p);
                }
                break; // first match within this root
            }
        }
    }

    if matches.is_empty() {
        return Err(Error::MissingConfigFile(path.clone()));
    }

    for p in matches {
        partial = load_partial(partial, p)?;
    }
}
```

## Drawbacks

- The resolved paths for user-global and user-workspace are deep (e.g.
  `$XDG_CONFIG_HOME/jp/config/.jp/config/skill/web.toml`). The `.jp/config`
  nesting inside `config/` looks redundant, but it is a direct consequence of
  treating `config_load_paths` entries uniformly across all roots. Avoiding this
  would require per-root load path configuration, which adds complexity for
  marginal benefit.

- The `config/` subdirectory convention for user-global and user-workspace roots
  is implicit — it is not configurable by the user. This is a deliberate
  trade-off to keep the design simple.

## Alternatives

### Resolve `config_load_paths` directly against all roots (no `config/` subdir)

Resolve `.jp/config` against `$XDG_CONFIG_HOME/jp/` directly, producing
`$XDG_CONFIG_HOME/jp/.jp/config/skill/web.toml`.

Rejected because it allows `config_load_paths` entries to conflict with existing
directory structure in those roots (e.g. a load path of `workspace` would clash
with `$XDG_CONFIG_HOME/jp/workspace/`).

### Hardcode `config/` as the search subdir for non-workspace roots

Rather than resolving `config_load_paths` against the non-workspace roots, just
search `<root>/config/<cfg_path>` directly (ignoring `config_load_paths`).

Rejected because it breaks the uniformity of the search — the same `--cfg`
argument would use different resolution logic depending on the root. It also
means `config_load_paths` has no effect on non-workspace roots, which is
surprising.

## Non-Goals

- **Changes to `config.toml` loading order.** The existing
  `load_partial_configs_from_files` function already loads `config.toml` from
  all three roots correctly. This RFD only extends `--cfg` path resolution to
  match that same multi-root behavior.

- **Multi-root resolution for `extends`.** The `extends` field is not affected
  by this change. Unlike `config_load_paths`, which is a runtime lookup
  mechanism where the merged list is searched at CLI invocation time, `extends`
  is resolved per-file during config loading. Each `config.toml` resolves its
  `extends` paths relative to its own parent directory, and the merge strategy
  (`schematic::merge::preserve`) ensures the first-loaded value wins rather than
  accumulating across sources. This means each config root already controls its
  own extensions independently — the user-global `config.toml` can extend files
  next to it, the workspace `config.toml` can extend files next to it, and so
  on. There is no cross-root resolution problem to solve.

- **New CLI flags or config schema changes.** The `config_load_paths` field and
  `--cfg` flag work as before. The only change is where the paths are searched.

## Risks and Open Questions

- **Directory creation.** Should `jp init` (or first use) create the `config/`
  subdirectory in user-global and user-workspace locations? Or rely on the user
  to create them manually?

- **Error reporting.** When `--cfg skill/web` fails to find a file, the error
  currently reports just the path. It should list all roots that were searched
  to help the user understand where to place the file.

- **`inherit = false` interaction.** If a workspace `config.toml` sets `inherit
  = false`, should that suppress loading `--cfg` files from the user-global
  root? Currently `inherit` only affects `config.toml` loading, not `--cfg`.
  This RFD preserves that behavior, but it may warrant a follow-up.

## Implementation Plan

This is a single-phase change, localized to `load_cli_cfg_args` in
`crates/jp_cli/src/lib.rs`:

1. Compute the three search roots (user-global + `config/`, workspace root,
   user-workspace + `config/`).
2. For each root, resolve `config_load_paths` and search for the `--cfg` path.
3. Collect and merge all matches in precedence order.
4. Improve the `MissingConfigFile` error to list searched paths.
5. Update `docs/configuration.md` to document the multi-root search behavior.

## References

- [Configuration documentation](../configuration.md) — current `--cfg` and
  `config_load_paths` behavior.
- `crates/jp_cli/src/lib.rs` — `load_cli_cfg_args`,
  `load_partial_configs_from_files`.
- `crates/jp_config/src/fs.rs` — `user_global_config_file` /
  `user_global_config_dir`.
- `crates/jp_config/src/util.rs` — `find_file_in_load_path`.
