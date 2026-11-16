# RFD 079: Config Sources and Load Order

- **Status**: Accepted
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-20
- **Required by**: [RFD 080]
- **Summary**: Documents JP's configuration sources, file probing order,
  `extends` resolution, `inherit` boundaries, and `config_load_paths` behavior.

## Summary

This guide describes how JP loads configuration at startup: which files it reads
and in what order, how `extends` directives and environment variables combine,
and how deferred loading via `config_load_paths` differs from implicit loading.
It is a reference for contributors and for RFDs that touch the config pipeline.

## File extensions

Config file paths in this guide use `{ext}` for the extensions JP probes, in
order:

- `toml`
- `json`
- `json5`
- `yaml`
- `yml`

The first matching file at a location wins; if none exists, that source is
skipped.

`json5` is probed but cannot be parsed: the loader registers only TOML,
JSON/JSONC, and YAML formats, so a `config.json5` file fails with
`NoMatchingFormat`.
Because `json5` precedes `yaml` in the probe order, it also shadows a sibling
`config.yaml`.

## Implicit loading

On every invocation, JP reads up to four on-disk sources, resolves each file's
`extends` directives, and applies `JP_CFG_*` environment variables on top.
The result is the invocation's **base partial**.

Sources are merged in order, with later sources overriding earlier ones:

1. **User-global config**

   - Linux: `~/.config/jp/config.{ext}` (respects `$XDG_CONFIG_HOME`)
   - macOS: `~/Library/Application Support/jp/config.{ext}`
   - Windows: `%APPDATA%\jp\config\config.{ext}`

   Setting `JP_GLOBAL_CONFIG_DIR` (tilde-expanded) overrides the platform
   directory; if no `config.{ext}` exists there, JP proceeds without a
   user-global config.
   Because the same directory is the user-global search root for [deferred
   loading](#implicit-loading-vs-deferred-loading), overriding
   `JP_GLOBAL_CONFIG_DIR` affects both mechanisms.

   This file is shared across all workspaces on the machine and typically holds
   personal defaults, the default model, and preferred providers.

2. **Workspace config**

   `<workspace-root>/.jp/config.{ext}`

   This file is commonly committed to version control alongside the project for
   team-shared defaults, project-specific tools, and code instructions.

3. **CWD overrides**

   `<cwd>/.jp.{ext}` (a dotfile in the directory itself, not
   `.jp/config.{ext}`), searched recursively from the current working directory
   up to the workspace root.
   Each directory's file is merged, and **deeper directories override shallower
   ones**.

   This supports subdirectory-scoped overrides in a monorepo.
   If a repository has `.jp.toml` at the root with general settings and
   `backend/.jp.toml` with backend-specific overrides, running `jp` from
   `backend/` applies both, with `backend/.jp.toml` taking precedence.

4. **User-workspace config**

   `<user-data-dir>/workspace/<workspace-name>-<workspace-id>/config.{ext}`,
   where `<user-data-dir>` is the platform's user data directory:

   - Linux: `~/.local/share/jp/` (respects `$XDG_DATA_HOME`)
   - macOS: `~/Library/Application Support/jp/`
   - Windows: `%LOCALAPPDATA%\jp\data\`

   This file is per-user and per-workspace, and is not committed to version
   control.
   It holds personal overrides for a single workspace without modifying the
   shared workspace config.

## `extends` directives

Any config file can include an `extends` directive to pull in additional files:

```toml
extends = [".jp/config.d/tools.toml", { path = ".jp/config.d/model.toml", strategy = "after" }]
```

Extended paths resolve relative to the directory of the file containing the
directive.
Each entry takes a merge strategy:

- **`before`** (default): merged before the parent file (lower precedence).
- **`after`**: merged after the parent file (higher precedence).

`extends` resolution is **recursive**: each extended file's own `extends`
directives are resolved as it loads.

The `extends` field has a schema default of `config.d/**/*`, but that default is
not applied during file loading: the loader reads each file's `extends` list
through `load_partial`, which does not inject `#[setting(default)]` values.
A file that omits `extends` extends nothing, and a sibling `config.d/` directory
is traversed only when the glob is declared explicitly.

Failure behavior:

- Missing non-glob targets log a warning and are skipped.
- Glob expansion errors log a warning and skip the failing entry.
- Cycles are rejected by checking an ancestor stack of canonicalized paths.
- A depth limit of 255 guards against unbounded recursion if cycle detection
  fails.

## Environment variables (`JP_CFG_*`)

Environment variables prefixed with `JP_CFG_` are the final step of implicit
loading and override every file source.
Each variable name maps to a dotted config path, and its value is parsed as the
target field's type:

```sh
JP_CFG_ASSISTANT_MODEL_ID=anthropic/claude-opus-4-6 jp query "..."
```

## `inherit` directive

A config file can set `inherit = false` to stop later file sources from being
merged on top of it.

As the loader walks the source order (user-global → workspace → CWD →
user-workspace), it checks the accumulated partial before loading each next
source.
Once `inherit` is `false`, file loading stops and remaining sources are skipped.

This lets an earlier layer lock in its settings: for example, setting `inherit =
false` in the workspace config prevents CWD and user-workspace files from
overriding it.

## Implicit loading vs. deferred loading

The sources above are **implicit**: JP loads them on every invocation to
establish baseline settings.
**Deferred loading** applies opt-in profiles only when requested via `--cfg
<name>`, resolving `<name>` (a relative file path, with or without extension)
against `config_load_paths`:

```toml
# .jp/config.toml
config_load_paths = [".jp/skills", ".jp/personas"]
```

With this setting, `jp query --cfg dev` searches `.jp/skills/` and
`.jp/personas/` for `dev.{ext}`, loads the first match, and merges it into the
current invocation's config.

A file in `.jp/skills/dev.toml` is ignored during implicit loading unless
another loaded file references it via `extends`.
Passing an explicit path such as `--cfg .jp/skills/dev.toml` bypasses
`config_load_paths` and resolves relative to the current working directory
rather than the workspace root.

### Multi-root search

`config_load_paths` merges across all config layers (`append_vec` with
deduplication), combining entries from user-global, workspace, CWD, and
user-workspace files.

JP resolves the merged paths against three search roots, in precedence order:

1. **User-global root**: `<user-config-dir>/config/`
2. **Workspace root**: `<workspace-root>/`
3. **User-workspace root**: `<user-data-dir>/workspace/<name>-<id>/config/`

Within each root, JP walks `config_load_paths` in order and takes the first
matching file.
Across roots, it collects every root's match and merges them in root precedence
order (user-global, then workspace, then user-workspace).
A single `--cfg dev` can therefore load both a team-shared `dev.toml` from the
workspace and a personal `dev.toml` override from the user-workspace, with the
user-workspace file taking precedence.
See [RFD 035] for the full multi-root resolution design and edge cases.

`config_load_paths` is read once from the base partial when constructing the
config pipeline.
Values set via `--cfg config_load_paths=...` or a conversation's config deltas
do not affect `--cfg <name>` lookups within the same invocation.

## Full load sequence

For commands that use the normal startup pipeline (excluding commands like `jp
init`), the load sequence is:

1. Load user-global `config.{ext}`, if present.
2. Recursively resolve its `extends` directives.
3. If the accumulated partial has `inherit = false`, stop loading file sources.
4. Load workspace `.jp/config.{ext}`, if present.
5. Recursively resolve its `extends` directives.
6. If the accumulated partial has `inherit = false`, stop loading file sources.
7. Load `.jp.{ext}` files from the current working directory up to the workspace
   root, resolving each file's `extends` directives.
   Shallower files merge first, and deeper files (closer to CWD) override them.
8. If the accumulated partial has `inherit = false`, stop loading file sources.
9. Load user-workspace `config.{ext}`, if present, and recursively resolve its
   `extends` directives.
10. Apply `JP_CFG_*` environment variables on top.
11. Record the resulting **base partial** for the invocation.
12. For continuing or forking invocations, load the conversation's
    `base_config.json` and event-stream `ConfigDelta`s.
    Any path cleared by those deltas and not set again ([RFD 070]) is also
    cleared from the base partial so the file layer cannot restore it; step 13
    can set it again.
13. Apply `--cfg` and `--no-cfg` ([RFD 038]) directives from left to right ([RFD
    008]).
14. Apply CLI shortcut flags (`--model`, `--reasoning`, etc.).
15. Validate the final resolved `AppConfig`.

Steps 1 through 11 produce the base partial; steps 12 through 14 layer on top.

In the pipeline implementation, steps 13 and 14 (`--cfg` and CLI flags) run
twice: once before step 12 to resolve `conversation.default_id`
(`ConfigPipeline::partial_without_conversation`), and again after step 12 with
the conversation layer included (`partial_with_conversation`).
Step 12 runs only once.

## References

- [RFD 008] (Ordered Tool Directives): left-to-right directive processing
  precedent that `--cfg` follows.
- [RFD 035] (Multi-Root Config Load Path Resolution): multi-root
  `config_load_paths` resolution design.
- [RFD 054] (Split Conversation Config and Events): structure of
  `base_config.json` and event-stream `ConfigDelta`s.
- [RFD 070] (Negative Config Deltas): how a delta records a cleared path.
- `crates/jp_cli/src/lib.rs`: `load_partial_configs_from_files` and
  `load_base_partial`.
- `crates/jp_config/src/util.rs`: `load_partials_with_inheritance`, `load_envs`,
  `load_config_file_with_extends`, and `load_partial_at_path_recursive`.
- `crates/jp_cli/src/config_pipeline.rs`: `ConfigPipeline` and
  `resolve_cfg_args` (deferred loading).

[RFD 008]: 008-ordered-tool-directives.md
[RFD 035]: 035-multi-root-config-load-path-resolution.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 070]: 070-negative-config-deltas.md
[RFD 080]: 080-editor-as-a-config-source.md
