# RFD 055: Tool Groups

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-20
- **Extended by**: [RFD 056]
- **Summary**: Introduces named tool groups for bulk `--tool`/`--no-tools` CLI
  toggling and optional exhaustive classification checks.

## Summary

This RFD introduces tool groups: named sets of tools that can be enabled or
disabled as a unit with `--tools GROUP` and `--no-tools GROUP`.
Tools declare their group membership via a `groups` field, and groups marked
`exhaustive = true` require every loaded tool to explicitly opt into or out of
the group.

## Motivation

Tools are configured individually in `conversation.tools.NAME`.
Named config fragments (loaded via `--cfg` or the `extends` field) bundle tool
configurations into reusable files, but there is no way to enable or disable a
*set* of tools from the CLI.

Disabling all filesystem write tools requires listing each tool by name:

```sh
jp query -T fs_create_file,fs_modify_file,fs_delete_file,fs_move_file "explain this code"
```

There is also no way to validate that *every* tool in the system is classified
as "write" or "not write".
A new MCP tool can silently retain write access when the user expects
`--no-tools write` to disable all write-capable tools.

A `--cfg disable_write_tools.toml` fragment works around the CLI verbosity, but
it is fragile: you must maintain a separate file that mirrors tool names, and
nothing checks that the file stays in sync when tools are added or removed.

Tool groups solve two problems:

1. **CLI shorthand**: `--no-tools write` disables all write tools in one flag.
2. **Exhaustive validation**: a group marked `exhaustive = true` requires every
   loaded tool to be classified as included or excluded, so no tool escapes
   unreviewed.

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

A group definition accepts one field:

- **`exhaustive`** (optional, default `false`): requires every enabled tool to
  be classified relative to this group (see [Exhaustive
  Validation](#exhaustive-validation)).

Groups do not list their members; tools declare their own membership.
Group names must not start with `!` (reserved for the exclusion shorthand) or
collide with tool names.
If a group and a tool share a name, JP exits with a config error at startup.

### Tool-Side Group Membership

Tools declare their group memberships via a `groups` field:

```toml
[conversation.tools.fs_create_file]
groups = ["write"]

[conversation.tools.fs_read_file]
groups = ["read", "!write"]
```

Each entry in the array is one of:

- A **string**: shorthand for included membership.
- A **`!`-prefixed string**: shorthand for excluded membership.
- A **structured entry**: `{ group = "NAME", membership = "include" | "exclude"
  }` (`membership` defaults to `"include"`).

These forms are equivalent:

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
Naming both `!write` and `write` in the same array is valid; the last entry
wins.

#### Membership States

Each (tool, group) pair has one of three states:

| State         | Meaning                         | How expressed                      |
| ------------- | ------------------------------- | ---------------------------------- |
| **Included**  | Tool is a member of the group   | `"write"` or `{ group = "write" }` |
| **Excluded**  | Tool is explicitly not a member | `"!write"` or `{ group = "write",  |
|               |                                 | membership = "exclude" }`          |
| **Undefined** | Tool has not been classified    | Group not mentioned in `groups`    |

Exhaustive validation relies on the distinction between **Excluded** (classified
as a non-member) and **Undefined** (not yet classified).

### Interaction with `*` Defaults

The `*` defaults section can set a default `groups` field:

```toml
[conversation.tools.'*']
groups = ["write"]

[conversation.tools.fs_read_file]
groups = ["!write", "read"]
```

This establishes a fail-closed baseline where all tools belong to the `write`
group unless they explicitly exclude themselves.
That makes `--no-tools write` safe by default: if a new tool is added and its
author forgets to classify it, it inherits `write` from `*` and is disabled
whenever write tools are disabled.

The `groups` field uses **merge-by-group-name** semantics at every merge
boundary (between `*` defaults and tool-level config, and across config layers
such as workspace config and `--cfg` overrides):

1. Start with the lower-priority `groups` array.
2. Remove any entry whose group name appears in the higher-priority array.
3. Append the higher-priority entries in their declared order.

A `--cfg` override that sets `groups = ["!write"]` on a tool replaces the
`write` entry from the workspace config's `*` defaults while retaining any other
inherited group entries.

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

Merging by group name avoids repeating exhaustive group memberships on every
tool.
Under replace semantics, any tool that sets `groups` would have to re-declare
every exhaustive group inherited from `*`.

### Exhaustive Validation

A group with `exhaustive = true` requires every enabled tool to be classified
relative to that group as either included or excluded.
If any enabled tool leaves the group undefined (not mentioned in the tool's
effective `groups` array after `*` merge), JP exits with a startup error.

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

Without a `*` baseline, every tool must explicitly list the exhaustive group in
its `groups` array.
Setting a baseline on `*` is a shorthand when most tools share a default
classification.

Exhaustive validation runs after all config layers are merged (config files,
`--cfg`, CLI flags) and before the query starts.

### CLI Interaction

`--tools` (`-t`) and `--no-tools` (`-T`) accept group names alongside tool
names:

```sh
# Enable all tools in the "read" and "cargo" groups
jp query -t read,cargo "fix the parser"

# Enable all tools, then disable the "write" group
jp query -t -T write "explain this code"

# Disable the "git" group
jp query -T git "review this design"
```

`--tools GROUP` enables and `--no-tools GROUP` disables all tools with
**included** membership in `GROUP`.
Because group names and tool names cannot collide, name resolution is
unambiguous.

### Interaction with Tool `enable` Field

Group flags respect each tool's `enable` setting:

| `enable` value   | `--tools GROUP`   | `--no-tools GROUP` |
| ---------------- | ----------------- | ------------------ |
| `None` (default) | Enabled           | Disabled           |
| `on` / `true`    | Enabled           | Disabled           |
| `off` / `false`  | Enabled           | Disabled           |
| `explicit`       | **Not enabled**   | Disabled           |
| `always`         | (already enabled) | **Not disabled**   |

- **`None` (default)**: treated as enabled, matching `is_none_or(Enable::is_on)`
  in the current implementation.

- **`explicit`**: not enabled by `--tools GROUP`, consistent with bare
  `--tools`.
  The tool must be named directly via `--tools TOOL`.

- **`allow_toggle = "if_named_or_group"`** (RFD 081): ignored by bare `--tools`,
  but enabled when one of its groups is passed to `--tools GROUP`.
  This lets a tool opt out of blanket enable-all while still responding to
  targeted group activation.
  The variant is defined in RFD 081; tool groups only need the `--tools GROUP`
  parser to produce `ToggleScope::NamedGroup`.

- **`always`**: cannot be disabled by any mechanism, including `--no-tools
  GROUP`, matching existing behavior for system-critical tools like
  `describe_tools`.

## Drawbacks

**Merge-by-name is a special case.** The `groups` field merges by group name
with `*` defaults, whereas scalar fields use replace semantics and other array
fields use `MergeableVec`'s append or replace strategies.
Avoiding re-declaration verbosity for exhaustive groups justifies the rule, but
it adds a merge behavior that users must learn and that must be documented
clearly.

## Alternatives

### Include/exclude lists on group definitions

Instead of tool-side membership, groups could declare their members:

```toml
[conversation.tools.groups.write]
include = ["fs_create_file", "fs_modify_file"]
exclude = ["fs_read_file", "cargo_check"]
```

Rejected because expressing membership on both the group and the tool creates
two places to declare the same relationship and requires conflict-resolution
rules.
Tool-side declaration keeps a single source of truth.

### Tags instead of groups

Tools could declare tags (`tags = ["read", "write"]`) filtered from the CLI via
`--tools @read`.
Tags do not support exhaustive validation or the include/exclude distinction.

### `exhaustive = "include" | "exclude"` variants

Instead of a boolean, `exhaustive` could accept `"include"` or `"exclude"` to
assign a default classification to unclassified tools.
These variants were dropped because `"include"` is equivalent to `false` (every
tool auto-classifies, so the check never fires) and `"exclude"` is already
expressed by the `*` defaults pattern (`'*'.groups = ["write"]` with per-tool
`"!write"` overrides).
A boolean is sufficient.

## Non-Goals

- **Group-level configuration.** Groups in this RFD are membership containers
  for CLI shorthand and exhaustive validation.
  Allowing groups to carry config defaults or overrides (e.g., group-wide `run`
  or `style` settings) is deferred to a future RFD.
- **Recursive groups.** Groups cannot contain other groups.
  Existing tool categories (read, write, git, cargo, github) are flat; nesting
  can be added in a future RFD if needed.

## Risks and Open Questions

**`*` merge semantics may surprise users.** Merging `groups` by name differs
from other config fields.
If this causes confusion in practice, we can fall back to replace semantics and
accept the verbosity cost for exhaustive groups.

## Implementation Notes

### `GroupMemberships` type

Merge-by-group-name is implemented on a dedicated `GroupMemberships` type rather
than as a third `MergeableVec` strategy, because extracting group names from
plain strings, `!`-prefixed strings, and structured entries is specific to
`groups` and does not generalize to other vector fields.

`GroupMemberships` wraps a `Vec<GroupEntry>` and provides:

- `merge_from(&mut self, other: &GroupMemberships)`: merges entries by group
  name.
- `included(&self) -> impl Iterator<Item = &str>`: yields included group names
  in declaration order.
- `is_classified(&self, group: &str) -> bool`: reports whether a group is
  explicitly included or excluded, for exhaustive validation.

`GroupEntry` is an enum supporting string shorthand (`"write"`, `"!write"`) and
structured form (`{ group, membership }`), with `group_name()` and
`is_excluded()` accessors.

The type uses `#[serde(transparent)]` to serialize as a plain JSON/TOML array.
A custom schematic merge function (`merge_group_memberships`) integrates it with
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
6. Validate at config load time that group and tool names do not collide and
   that every referenced group is defined.

### Phase 2: Exhaustive validation

1. After full config merge, iterate all enabled tools and compute their
   effective groups (tool-level merged with `*` defaults).
2. For each group with `exhaustive = true`, verify every enabled tool is
   classified (included or excluded).
   Exit with an error listing unclassified tools if any are found.

### Phase 3: CLI integration

1. Extend `--tools` / `--no-tools` name resolution to check groups (unambiguous
   since group/tool name collisions are a config error).
2. When a group name matches, expand it to enable or disable all tools with
   included membership in that group.
3. Respect each tool's `allow_toggle` policy: `if_named` tools are not activated
   by group enable; locked (`never`) tools are not flipped by group directives.
   `if_named_or_group` tools (RFD 081) respond to group activation.
4. Produce `ToggleScope::NamedGroup` from the `--tools GROUP` / `--no-tools
   GROUP` parser so the directive engine applies the group scope.
5. Update existing tests in `query_tests.rs` to cover group interactions.

## References

- [Configuration architecture]: progressive complexity design principles
- `MergeableVec` (`crates/jp_config/src/types/vec.rs`): existing array merge
  strategies in the config system
- `crates/jp_config/src/conversation/tool.rs`: `ToolConfig`, `ToolsConfig`,
  `ToolsDefaultsConfig`, `Enable`
- `crates/jp_cli/src/cmd/query.rs`: `apply_enable_tools`, `--tools` /
  `--no-tools` handling
- [RFD 042]: tool options (per-tool runtime configuration, complementary
  feature)

[Configuration architecture]: ../configuration.md
[RFD 042]: 042-tool-options.md
[RFD 056]: 056-group-configuration-defaults.md
