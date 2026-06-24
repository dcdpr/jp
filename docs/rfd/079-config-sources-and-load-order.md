# RFD 079: Config Sources and Load Order

- **Status**: Accepted
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-20
- **Requires**: [RFD 038]
- **Required by**: [RFD 080]

## Summary

This guide describes how JP loads configuration at startup: which files are
read, in what order, how extension and environment layers combine, and how
deferred loading via `config_load_paths` differs from implicit loading.
It is reference material for contributors and for other RFDs that touch the
config pipeline.

## File extensions

All config file paths in this guide use `{ext}` as shorthand for the list of
supported extensions:

- `toml`
- `json`
- `json5`
- `yaml`
- `yml`

Extensions are tried in that order at each location.
The first existing file wins.
If no file exists at a given source, that source contributes nothing.

## Implicit loading

On every invocation, JP reads up to four on-disk sources, resolves each file's
`extends` directives, and applies `JP_CFG_*` environment variables on top.
The final result is the **base partial** for the invocation.

Source order (earlier sources are merged first; later sources override):

1. **User-global config**

   - Linux: `~/.config/jp/config.{ext}` (respects `$XDG_CONFIG_HOME`)
   - macOS: `~/Library/Application Support/jp/config.{ext}`
   - Windows: `%APPDATA%\jp\config\config.{ext}`

   The containing directory can be overridden via the `JP_GLOBAL_CONFIG_DIR` env
   var (tilde-expanded); `config.{ext}` is then loaded from that directory
   instead of the platform default.

   File extensions are tried in order; if no `config.{ext}` exists in the
   directory, JP silently proceeds without a user-global config.

   Shared across all workspaces on the machine.
   Typical use: personal defaults, default model, preferred providers.

   The same directory also serves as the user-global search root for deferred
   loading (see [Implicit loading vs. deferred
   loading](#implicit-loading-vs-deferred-loading)), so overriding the env var
   affects both mechanisms consistently.

2. **Workspace config**

   `<workspace-root>/.jp/config.{ext}`

   Commonly committed to version control alongside the project.
   Typical use: team-shared defaults, project-specific tools, code instructions.

3. **CWD overrides**

   `<cwd>/.jp.{ext}`, searched recursively from the current working directory up
   to the workspace root.
   Each directory's file is merged; **deeper directories override shallower
   ones**.

   Typical use: subdirectory-scoped overrides in a monorepo.
   For example, the workspace might have `.jp.toml` at the repository root with
   broadly applicable settings, and a sub-directory like `backend/.jp.toml` with
   backend-specific overrides.
   Running `jp` from within `backend/` applies both, with `backend/.jp.toml`
   taking precedence over the root file.

   Note the different file naming: CWD config is `.jp.{ext}` (a dotfile at the
   directory level), not `.jp/config.{ext}`.

4. **User-workspace config**

   `<user-data-dir>/workspace/<workspace-name>-<workspace-id>/config.{ext}`,
   where `<user-data-dir>` is the platform's user data directory:

   - Linux: `~/.local/share/jp/` (respects `$XDG_DATA_HOME`)
   - macOS: `~/Library/Application Support/jp/`
   - Windows: `%LOCALAPPDATA%\jp\data\`

   Per-workspace, per-user, not committed.
   Typical use: personal overrides a user wants applied only to a specific
   workspace without modifying the shared workspace config.

## `extends` directives

Any config file can include an `extends` directive pulling in additional files:

```toml
extends = [".jp/config.d/tools.toml", { path = ".jp/config.d/model.toml", strategy = "after" }]
```

Extended paths are resolved relative to the directory of the file containing the
directive.
Each entry has a strategy:

- **`before`** (default): the extended file is merged before the parent file
  (lower precedence).
- **`after`**: the extended file is merged after the parent file (higher
  precedence).

Extends is **recursive** — each extended file's own `extends` directives are
resolved when it's loaded.
A chain can go many layers deep.

The default `extends` value is the glob `config.d/**/*`, which auto-loads any
files dropped into a sibling `config.d/` directory.
This lets users split config into many small files without editing the main
config's `extends` list.

Failure behavior:

- Missing non-glob targets log a warning and continue.
- Per-entry glob expansion errors log and skip that entry.
- Cycles are rejected via an ancestor-stack check on canonicalized paths.
- A depth cap (currently 255) acts as a safety net if cycle detection fails.

## Environment variables (`JP_CFG_*`)

After all file sources are loaded and merged, environment variables prefixed
with `JP_CFG_` apply on top.
The variable name maps to a dotted config path; the value is parsed as the
field's expected type.

Example:

```sh
JP_CFG_ASSISTANT_MODEL_ID=anthropic/claude-opus-4-6 jp query "..."
```

Env vars are the last step of implicit loading and override every file source.

## `inherit` directive

A config file can set `inherit = false` to stop further files from being merged
on top of it.

Processing: the loader iterates source files in order (user-global → workspace
→ CWD → user-workspace).
Before merging each next source, it checks whether the accumulated state has
`inherit = false`.
If so, processing stops and no later sources are merged.

This provides a way for a less-specific layer to declare itself authoritative,
preventing more-specific overrides.
For example, a workspace config that sets `inherit = false` prevents CWD and
user-workspace files from overriding its values.

## Implicit loading vs. deferred loading

The sources above are **implicit** — loaded at every invocation without user
action.
In contrast, **deferred loading** via `--cfg <name>` requires explicit user
action and resolves through a separate mechanism: `config_load_paths`.

`config_load_paths` is a list of directory paths that JP searches when resolving
`--cfg <name>`, where `<name>` is a file path relative to the config load paths,
with or without extension.
For example:

```toml
# .jp/config.toml
config_load_paths = [".jp/skills", ".jp/personas"]
```

With this setting, `jp query --cfg dev` searches for `dev.{ext}` in
`.jp/skills/` and `.jp/personas/`, loads the first match, and merges it into the
current invocation's config.

### Multi-root search

`config_load_paths` merges across all config layers (`append_vec` + dedup), so
entries from user-global, workspace, CWD, and user-workspace files all
contribute to the final search list.

The merged entries are resolved against three search roots:

1. **User-global root** — `<user-config-dir>/config/`
2. **Workspace root** — `<workspace-root>/`
3. **User-workspace root** — `<user-data-dir>/workspace/<name>-<id>/config/`

For each root, JP walks the `config_load_paths` entries in order; the first
matching file within a root wins.
Across roots, all matches are collected and merged in root precedence order
(user-global first, workspace next, user-workspace last).

This means a single `--cfg dev` can resolve to multiple files across roots —
for example, a team-shared `dev.toml` in the workspace and a personal override
`dev.toml` in the user-workspace.
Both get merged, with the user-workspace file taking precedence.

See [RFD 035] for the full multi-root resolution design and edge cases.

Note: `config_load_paths` is read once from the base partial when the config
pipeline is constructed.
Values set via `--cfg config_load_paths=...` or a conversation's config deltas
don't affect `--cfg <name>` lookups within the same invocation.

### Key differences from implicit loading

- **Implicit loading** happens every invocation; `config_load_paths` is not
  involved.
- **Deferred loading** happens only when the user passes `--cfg <name>`.
  The paths in `config_load_paths` are the search space for that resolution.
- A file in `.jp/skills/dev.toml` is *not* loaded unless the user types `--cfg
  dev` (or references it via an `extends` directive, or passes an explicit path
  as `--cfg .jp/skills/dev.toml`).
  Explicit paths are resolved from the current working directory, not the
  workspace root.

This separation is deliberate: implicit loading provides universal baseline
settings; `config_load_paths` enables a library of opt-in profiles that only
apply when explicitly requested.

## Full load sequence

For commands that go through the normal startup pipeline (e.g. excluding `jp
init`), the sequence is:

1. Load user-global `config.{ext}` if it exists.
2. Resolve its `extends` directives (recursively).
3. If accumulated state has `inherit = false`, stop.
   Else continue.
4. Load workspace `.jp/config.{ext}` if it exists.
5. Resolve its `extends` directives (recursively).
6. If accumulated state has `inherit = false`, stop.
   Else continue.
7. Load CWD `.jp.{ext}` files, recursively from CWD up to workspace root.
   Resolve each file's `extends` directives.
   Shallower files are merged first; deeper (closer to CWD) files override.
8. If accumulated state has `inherit = false`, stop.
   Else continue.
9. Load user-workspace `config.{ext}` if it exists.
   Resolve its `extends` directives.
10. Apply `JP_CFG_*` environment variables on top.
11. The result is the **base partial** for this invocation.
12. Load the conversation's `base_config.json` and event-stream `ConfigDelta`s
    (for continuing or forking invocations only).
13. Apply `--cfg` and `--no-cfg` ([RFD 038]) directives left-to-right ([RFD
    008]).
14. Apply CLI shortcut flags (`--model`, `--reasoning`, etc.).
15. Validate the final resolved `AppConfig`.

Steps 1–11 produce the base partial; steps 12–14 layer on top.

Note: in the actual pipeline, steps 13–14 (`--cfg` and CLI flags) run twice —
once before step 12 to resolve `conversation.default_id`, once after step 12
with the conversation layer included.
Step 12 itself runs only once.
This two-phase split is an implementation detail that RFDs touching the config
pipeline may care about; see `ConfigPipeline::partial_without_conversation` and
`partial_with_conversation`.

## References

- [RFD 008]: Ordered Tool Directives — precedent for left-to-right directive
  processing (`--cfg` follows the same pattern).
- [RFD 035]: Multi-Root Config Load Path Resolution — full design of
  `config_load_paths` resolution across roots.
- [RFD 054]: Split Conversation Config and Events — how `base_config.json` and
  event-stream `ConfigDelta`s are structured.
- `crates/jp_cli/src/lib.rs` — `load_partial_configs_from_files` and
  `load_base_partial`.
- `crates/jp_config/src/util.rs` — `load_partials_with_inheritance`,
  `load_envs`, `load_config_file_with_extends`,
  `load_partial_at_path_recursive`.
- `crates/jp_cli/src/config_pipeline.rs` — `ConfigPipeline` and
  `resolve_cfg_args` (the deferred-loading side).

[RFD 008]: 008-ordered-tool-directives.md
[RFD 035]: 035-multi-root-config-load-path-resolution.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 080]: 080-editor-as-a-config-source.md
