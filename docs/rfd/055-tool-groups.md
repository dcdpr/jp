# RFD 055: Tool Groups

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-20
- **Extended by**: [RFD 056](056-group-configuration-defaults.md)

## Summary

This RFD introduces tool groups: named sets of tools that can be enabled or
disabled as a unit via `--tools GROUP` / `--no-tools GROUP`. Groups support
exhaustive validation, ensuring every tool is classified relative to a group.
Group membership is declared on the tool side via a `groups` field.

## Motivation

Tools are configured individually in `conversation.tools.NAME`. Named config
fragments (loaded via `--cfg` or the `extends` field) bundle tool configurations
into reusable files, but there is no way to enable or disable a *set* of tools
from the CLI.

Today, disabling all filesystem write tools requires:

```sh
jp query -T fs_create_file,fs_modify_file,fs_delete_file,fs_move_file "explain this code"
```

And ensuring that *every* tool in the system is classified as "write" or "not
write" is impossible — there is no validation mechanism. A new MCP tool could
silently retain write access when the user expected `--no-tools write` to
disable all write-capable tools.

A `--cfg disable_write_tools.toml` workaround exists, but it's fragile — you
maintain a separate file that mirrors tool names, and nothing validates that the
file stays in sync when tools are added or removed.

Tool groups solve two problems:

1. **CLI shorthand**: `--no-tools write` disables all write tools in one flag.
2. **Exhaustive validation**: a group marked `exhaustive = true` guarantees that
   every loaded tool has been explicitly classified relative to that group. No
   tool escapes unreviewed.

## Design

### Group Definition

Groups are defined at `conversation.tools.groups.NAME`:

```toml
[conversation.tools.groups]
write = { exhaustive = true }
read = {}
git = {}
cargo = {}
```

A group definition contains:

- **`exhaustive`** (optional, default `false`): whether every tool must be
  classified relative to this group. See [Exhaustive
  Validation](#exhaustive-validation).

Groups do not list their members. Membership is declared on the tool side (see
below).

Group names must not collide with tool names. If a group and a tool share a
name, JP exits with a config error at startup.

Group names must not start with `!` (reserved for the exclusion shorthand).

### Tool-Side Group Membership

Tools declare their group memberships via a `groups` field:

```toml
[conversation.tools.fs_create_file]
groups = ["write"]

[conversation.tools.fs_read_file]
groups = ["read", "!write"]
```

Each entry in the array is either:

- A **string** — shorthand for included membership.
- A **`!`-prefixed string** — shorthand for excluded membership.
- A **structured entry** — `{ group = "NAME", membership = "include" | "exclude"
  }` for when the long form is preferred.

The three forms are equivalent:

```toml
# These all mean "included in write":
groups = ["write"]
groups = [{ group = "write" }]
groups = [{ group = "write", membership = "include" }]

# These all mean "excluded from write":
groups = ["!write"]
groups = [{ group = "write", membership = "exclude" }]
```

Referencing a group name that does not exist in `conversation.tools.groups` is a
config error.

Naming both `!write` and `write` is valid, last-defined wins.

#### Membership States

For any (tool, group) pair, there are exactly three states:

| State         | Meaning                         | How expressed                            |
|---------------|---------------------------------|------------------------------------------|
| **Included**  | Tool is a member of the group   | `"write"` or `{ group = "write" }`       |
| **Excluded**  | Tool is explicitly not a member | `"!write"` or `{ group = "write",        |
|               |                                 | membership = "exclude" }`                |
| **Undefined** | Tool has not been classified    | Group not mentioned in `groups`          |

The distinction between excluded and undefined is what makes exhaustive
validation meaningful.

### Interaction with `*` Defaults

The `*` defaults section can set a default `groups` field:

```toml
[conversation.tools.'*']
groups = ["write"]

[conversation.tools.fs_read_file]
groups = ["!write", "read"]
```

This establishes a fail-closed baseline: all tools default to the write group. A
tool that is *not* a write tool must explicitly exclude itself. This means
`--no-tools write` is safe by default — if a new tool is added and the author
forgets to classify it, it lands in the write group and gets disabled when write
tools are disabled. No tool silently escapes with write access.

The `groups` field uses **merge-by-group-name** semantics at every merge
boundary — between `*` defaults and tool-level config, and between config file
layers (workspace config, `--cfg` overrides, etc.).

The merge rule is the same at every boundary:

1. Start with the lower-priority groups array.
2. Remove any entries whose group name appears in the higher-priority array (the
   higher-priority side overrides those).
3. Append all higher-priority entries.

Retained lower-priority entries come first. Higher-priority entries follow in
their declared order.

This means a `--cfg` override that sets `groups = ["!write"]` on a tool will
override the `write` entry from the workspace config's `*` defaults, while
retaining any other group entries that were already present.

**Example:**

```toml
# Defaults
[conversation.tools.'*']
groups = ["write"]

# fs_read_file overrides write, adds read
[conversation.tools.fs_read_file]
groups = ["!write", "read"]

# github_issues inherits "write" from *, adds github
[conversation.tools.github_issues]
groups = ["github"]

# cargo_check: inherits "write" from *, no tool-level groups override
# (if it has no groups field at all, it inherits ["write"] from *)

# Effective groups:
# fs_read_file:   ["!write", "read"]   (tool replaced *'s write with !write, added read)
# github_issues:  ["write", "github"]  (*'s write retained, tool's github appended)
# cargo_check:    ["write"]            (inherited from *)
```

This merge behavior avoids the verbosity problem with exhaustive groups. Without
it, every tool that sets its own `groups` would need to re-declare all
exhaustive group memberships — exactly the kind of error-prone repetition that
exhaustive validation is meant to prevent.

### Exhaustive Validation

A group with `exhaustive = true` requires that every enabled tool has been
classified relative to that group — either included or excluded. If any tool has
the group in undefined state (not mentioned in the tool's effective `groups`
array after `*` merge), JP exits with a startup error.

```toml
[conversation.tools.groups.write]
exhaustive = true

[conversation.tools.'*']
groups = ["write"] # baseline: all tools in write

[conversation.tools.fs_read_file]
groups = ["!write", "read"] # override: excluded from write

[conversation.tools.some_new_mcp_tool]
source = "mcp.my_server"
# inherits "write" from * → classified → OK
```

Without the `*` baseline, every tool must explicitly mention the exhaustive
group in its `groups` array. Both approaches are valid — the `*` baseline is a
convenience for the common pattern where most tools share a default
classification.

Exhaustive validation runs after all config layers are merged (config files,
`--cfg`, CLI flags) but before the query starts.

### CLI Interaction

`--tools` (`-t`) and `--no-tools` (`-T`) accept group names in addition to tool
names:

```sh
# Enable all tools in the "read" and "cargo" groups
jp query -t read,cargo "fix the parser"

# Enable all tools, then disable the "write" group
jp query -t -T write "explain this code"

# Disable the "git" group
jp query -T git "review this design"
```

Since group names and tool names cannot collide, name resolution is unambiguous.

`--tools GROUP` enables all tools with **included** membership in that group.
`--no-tools GROUP` disables all tools with included membership in that group.

### Interaction with Tool `enable` Field

The existing `enable` field on tool config controls tool activation behavior.
Groups interact with it as follows:

| `enable` value      | `--tools GROUP`   | `--no-tools GROUP`   |
|---------------------|-------------------|----------------------|
| `None` (default)    | Enabled           | Disabled             |
| `on` / `true`       | Enabled           | Disabled             |
| `off` / `false`     | Enabled           | Disabled             |
| `explicit`          | **Not enabled**   | Disabled             |
| `explicit_or_group` | Enabled           | Disabled             |
| `always`            | (already enabled) | **Not disabled**     |

- **`None` (default)**: the tool has no explicit enable setting. Treated as
  enabled (consistent with `is_none_or(Enable::is_on)` in the current
  implementation).

- **`explicit`**: not enabled by `--tools GROUP`, consistent with not being
  enabled by bare `--tools`. Requires being named directly: `--tools TOOL`.

- **`explicit_or_group`** (new variant): like `explicit`, the tool is not
  enabled by bare `--tools`. But unlike `explicit`, it *is* enabled when a group
  it belongs to is activated via `--tools GROUP`. This allows tools that should
  not be swept up by blanket enable-all but should respond to targeted group
  activation.

- **`always`**: cannot be disabled by any mechanism, including `--no-tools
  GROUP`. Consistent with existing behavior for system-critical tools like
  `describe_tools`.

## Drawbacks

**Merge-by-name is a special case.** The `groups` field uses merge-by-name
semantics with `*` defaults, while scalar fields use replace semantics and other
array fields use `MergeableVec`'s append/replace strategies. This must be
documented clearly. The justification (avoiding re-declaration verbosity with
exhaustive groups) is sound, but it adds a concept users must learn.

**New `Enable` variant.** Adding `explicit_or_group` increases the surface area
of an already nuanced enum. Users who don't use groups will never encounter it,
but it is another value to document and maintain.

## Alternatives

### Include/exclude lists on group definitions

Instead of tool-side membership, groups could declare their members:

```toml
[conversation.tools.groups.write]
include = ["fs_create_file", "fs_modify_file"]
exclude = ["fs_read_file", "cargo_check"]
```

This was rejected because it creates dual membership paths — membership
expressed both on the group and on the tool. Conflicts between the two require
resolution rules. Tool-side declaration is the single source of truth.

### Tags instead of groups

Tools could declare tags (`tags = ["read", "write"]`) and the CLI could filter
by tag (`--tools @read`). This is similar to the chosen design but does not
support exhaustive validation or the include/exclude distinction.

### `exhaustive = "include" | "exclude"` variants

Instead of just `true | false`, exhaustive could default unclassified tools to
included or excluded. These were dropped because `"include"` is functionally
equivalent to `false` (the check never fires — every tool auto-classifies) and
`"exclude"` is already the natural behavior of the `*` defaults pattern
(`'*'.groups = ["write"]` with per-tool `"!write"` overrides). The boolean is
sufficient.

## Non-Goals

- **Group-level configuration.** Groups in this RFD are purely membership
  containers for CLI shorthand and exhaustive validation. Allowing groups to
  carry config defaults or overrides (e.g., group-wide `run` or `style`
  settings) is a natural extension but is deferred to a future RFD.
- **Recursive groups.** Groups cannot contain other groups. The real-world
  taxonomy is shallow (read, write, git, cargo, github). If deeper nesting is
  needed, it can be added in a future RFD.

## Risks and Open Questions

**`*` merge semantics may surprise users.** The merge-by-name behavior for
`groups` differs from other fields. If this causes confusion in practice, we
could fall back to replace semantics and accept the verbosity cost for
exhaustive groups.

**`Enable` enum growth.** The enum now has five variants (`on`, `off`,
`explicit`, `explicit_or_group`, `always`). If future features add more
activation modes, the enum may need restructuring. For now, five variants is
manageable.

## Implementation Notes

### `GroupMemberships` type

The merge-by-group-name behavior is implemented as a dedicated
`GroupMemberships` type rather than extending `MergeableVec` with a third
strategy. The key extraction logic (parsing group names from strings,
`!`-prefixed strings, and structured entries) is specific to the groups type and
doesn't generalize to other vec fields.

`GroupMemberships` wraps a `Vec<GroupEntry>` and provides:

- `merge_from(&mut self, other: &GroupMemberships)` — the merge-by-name
  operation.
- `included(&self) -> impl Iterator<Item = &str>` — iterate included group names
  in declaration order.
- `is_classified(&self, group: &str) -> bool` — check if a group is mentioned
  (included or excluded), for exhaustive validation.

`GroupEntry` is an enum supporting string shorthand (`"write"`, `"!write"`) and
structured form (`{ group, membership }`), with `group_name()` and
`is_excluded()` accessors.

The type uses `#[serde(transparent)]` to serialize as a plain JSON/TOML array. A
custom schematic merge function (`merge_group_memberships`) integrates it with
the config system.

## Implementation Plan

### Phase 1: Types and membership

1. Implement `GroupMemberships`, `GroupEntry`, and `Membership` types with
   merge-by-group-name semantics and the `merge_group_memberships` merge
   function.
2. Add `ToolGroupConfig` struct with `exhaustive` field (no config sections).
3. Add `groups: IndexMap<String, ToolGroupConfig>` to `ToolsConfig` (as a named
   field before the flattened `tools` field, following the same pattern as
   `defaults`).
4. Add `groups` field to both `ToolConfig` and `ToolsDefaultsConfig`, using the
   `GroupMemberships` type.
5. Update `AssignKeyValue`, `PartialConfigDelta`, and `ToPartial`
   implementations for `PartialToolsConfig` to handle the new `groups` field.
6. Validate at config load time: no group/tool name collisions, no references to
   undefined groups.

### Phase 2: Exhaustive validation

1. After full config merge, iterate all enabled tools and compute their
   effective groups (tool-level merged with `*` defaults).
2. For each group with `exhaustive = true`, verify every enabled tool is
   classified (included or excluded). Exit with an error listing unclassified
   tools if any are found.

### Phase 3: CLI integration

1. Extend `--tools` / `--no-tools` name resolution to check groups (unambiguous
   since group/tool name collisions are a config error).
2. When a group name is matched, expand to enable/disable all tools with
   included membership in that group.
3. Respect `enable` field variants: `explicit` tools are not activated by group
   enable; `always` tools are not disabled by group disable.
4. Add `Enable::ExplicitOrGroup` variant.
5. Update existing tests in `query_tests.rs` to cover group interactions.

## References

- [Configuration architecture] — progressive complexity design principles
- `MergeableVec` (`crates/jp_config/src/types/vec.rs`) — existing array merge
  strategies in the config system
- `crates/jp_config/src/conversation/tool.rs` — `ToolConfig`, `ToolsConfig`,
  `ToolsDefaultsConfig`, `Enable`
- `crates/jp_cli/src/cmd/query.rs` — `apply_enable_tools`, `--tools` /
  `--no-tools` handling
- [RFD 042] — tool options (per-tool runtime configuration, complementary
  feature)

[Configuration architecture]: ../configuration.md
[RFD 042]: 042-tool-options.md
