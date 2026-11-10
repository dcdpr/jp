# RFD 035: Multi-Root Config Load Path Resolution

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-08
- **Summary**: Searches user-global, workspace, and user-workspace
  `config_load_paths` roots when resolving `--cfg` fragments and merges all
  matches in precedence order.

## Summary

This RFD resolves relative `--cfg` paths across all three configuration roots
(user-global, workspace, and user-workspace) instead of only the workspace root.
When a fragment exists in multiple roots, JP loads and merges all matching files
in precedence order.

## Motivation

JP loads `config.toml` from three locations, in precedence order:

1. **User-global** (`$XDG_CONFIG_HOME/jp/config.toml`): private to the user and
   shared across all workspaces.
2. **Workspace** (`<workspace_root>/.jp/config.toml`): shared with the team via
   version control.
3. **User-workspace** (`$XDG_DATA_HOME/jp/workspace/<id>/config.toml`): private
   to the user and scoped to a single workspace.

This layering lets teams define shared defaults while individual users override
them privately.
The `--cfg` flag extends this model by loading named config fragments (such as
`skill/web` or `personas/dev`) from directories listed in `config_load_paths`.

However, `--cfg` resolution searches `config_load_paths` only relative to the
workspace root.
Placing `skill/web.toml` in the user-global config directory fails because JP
never searches that root.
Users cannot define personal `--cfg` fragments that apply across all workspaces
or to a single workspace without either modifying the shared workspace config or
passing the full path on every invocation (`--cfg $XDG_CONFIG_HOME/jp/...`).

## Design

### Current behavior

`load_cli_cfg_args` in `crates/jp_cli/src/lib.rs` handles `--cfg` path
arguments.
When the argument is neither an existing file nor a key-value pair, it iterates
`config_load_paths` and resolves each entry against `workspace.root()` only:

```rust
let config_load_paths = workspace.iter().flat_map(|w| {
    partial.config_load_paths.iter().flatten().filter_map(|p| {
        Utf8PathBuf::try_from(p.to_path(w.root()))
        // ...
    })
});
```

It then calls `find_file_in_load_path` for each resolved path and stops at the
first match (`break`).

### Proposed behavior

JP resolves `config_load_paths` against all three configuration roots, searching
each root from lowest to highest precedence.
When multiple roots match, JP loads and merges all matching files in precedence
order:

| Precedence  | Source         | Resolution root                            |
| ----------- | -------------- | ------------------------------------------ |
| 1 (lowest)  | User-global    | `$XDG_CONFIG_HOME/jp/config/`              |
| 2           | Workspace      | `<workspace_root>/`                        |
| 3 (highest) | User-workspace | `$XDG_DATA_HOME/jp/workspace/<id>/config/` |

The user-global and user-workspace roots resolve entries under a `config/`
subdirectory so `config_load_paths` entries do not collide with non-config
entries at the top level (`workspace/`, the `storage` symlink, `conversations/`,
and others).
The workspace root does not need a `config/` subdirectory because
`config_load_paths` entries such as `.jp/config` are already scoped by
convention.

### Example

Given `config_load_paths = [".jp/config"]` and `--cfg skill/web`, JP searches:

1. `$XDG_CONFIG_HOME/jp/config/.jp/config/skill/web.toml`
2. `<workspace_root>/.jp/config/skill/web.toml`
3. `$XDG_CONFIG_HOME/jp/workspace/<id>/config/.jp/config/skill/web.toml`

If files exist at positions 1 and 2, JP loads both and merges the workspace file
(2) on top of the user-global file (1).
If only one file exists, JP loads it as-is.

### Merge behavior

Within a single root, the first entry in `config_load_paths` that produces a
match wins and remaining entries in that root are skipped.
Across roots, JP merges all matches in precedence order using `load_partial`.

### Code changes

The change is localized to the `KeyValueOrPath::Path` branch in
`load_cli_cfg_args` (`crates/jp_cli/src/lib.rs`).
The function already receives `workspace: Option<&Workspace>`, which provides
`root()` and `user_storage_path()`, while `user_global_config_dir()` provides
the user-global path:

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

- Resolved paths for user-global and user-workspace roots are deep (for example,
  `$XDG_CONFIG_HOME/jp/config/.jp/config/skill/web.toml`).
  Nesting `.jp/config` inside `config/` looks redundant, but it follows directly
  from applying `config_load_paths` entries uniformly across all roots.
  Avoiding the extra nesting would require per-root load path configuration,
  adding complexity for little benefit.

- The `config/` subdirectory for user-global and user-workspace roots is
  hardcoded rather than user-configurable, trading flexibility for a simpler
  configuration model.

## Alternatives

### Resolve `config_load_paths` directly against all roots

Resolving `.jp/config` directly against `$XDG_CONFIG_HOME/jp/` without a
`config/` subdirectory produces `$XDG_CONFIG_HOME/jp/.jp/config/skill/web.toml`.
This approach was rejected because `config_load_paths` entries could collide
with existing directories in those roots (for example, a load path of
`workspace` would clash with `$XDG_CONFIG_HOME/jp/workspace/`).

### Ignore `config_load_paths` in non-workspace roots

Searching `<root>/config/<cfg_path>` directly in non-workspace roots while
ignoring `config_load_paths` avoids nesting `.jp/config` under `config/`.
This approach was rejected because the same `--cfg` argument would follow
different resolution rules depending on the root, and `config_load_paths` would
have no effect outside the workspace root.

## Non-Goals

- **Changes to `config.toml` loading order.** `load_partial_configs_from_files`
  already loads `config.toml` from all three roots.
  This RFD only brings `--cfg` path resolution in line with that multi-root
  behavior.

- **Multi-root resolution for `extends`.** Unlike `config_load_paths`, whose
  merged list is searched at CLI invocation time, `extends` is resolved per file
  during config loading.
  Each `config.toml` resolves `extends` paths relative to its own parent
  directory, and `schematic::merge::preserve` keeps the first-loaded value
  rather than accumulating across sources.
  Each root already controls its own extensions independently (the user-global
  and workspace `config.toml` files each extend files relative to their own
  directory).

- **New CLI flags or config schema changes.** `config_load_paths` and `--cfg`
  keep their existing behavior and schema; only the search roots change.

## Risks and Open Questions

- **Directory creation.** Should `jp init` (or first use) create the `config/`
  subdirectory in the user-global and user-workspace roots, or rely on the user
  to create it manually?

- **Error reporting.** When `--cfg skill/web` finds no matching file, the error
  currently reports only the requested path.
  Listing all searched roots in the error message would show the user where to
  place the file.

- **`inherit = false` interaction.** If a workspace `config.toml` sets `inherit
  = false`, should that suppress loading `--cfg` files from the user-global
  root?
  `inherit` only affects `config.toml` loading, not `--cfg`.
  This RFD preserves that behavior, which can be revisited in a follow-up.

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

- [Configuration documentation]: current `--cfg` and `config_load_paths`
  behavior.
- `crates/jp_cli/src/lib.rs`: `load_cli_cfg_args`,
  `load_partial_configs_from_files`.
- `crates/jp_config/src/fs.rs`: `user_global_config_file` /
  `user_global_config_dir`.
- `crates/jp_config/src/util.rs`: `find_file_in_load_path`.

[Configuration documentation]: ../configuration.md
