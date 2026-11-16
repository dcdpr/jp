# RFD 056: Group Configuration Defaults

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-20
- **Extends**: [RFD 055]
- **Extended by**: [RFD 057]
- **Summary**: Adds a `defaults` section to tool groups so member tools inherit
  shared settings without per-tool repetition.

## Summary

This RFD adds a `defaults` section to tool group definitions so groups can
supply fallback configuration (`enable`, `run`, `result`, `style`, and
`questions`) to their member tools.
A tool's `groups` array order determines which group's defaults take precedence.

## Motivation

[RFD 055] introduces tool groups for CLI shorthand and exhaustive validation,
but groups are membership containers with no configuration of their own.
Configuring shared behavior across a set of tools still requires repeating the
settings on every tool:

```toml
[conversation.tools.git_commit]
run = "unattended"
style.inline_results = "off"

[conversation.tools.git_diff]
run = "unattended"
style.inline_results = "off"

[conversation.tools.git_add_intent]
run = "unattended"
style.inline_results = "off"
```

A `--cfg` file can bundle these definitions together, but each tool still
repeats the same fields, and changing the shared behavior requires updating
every tool.

With group defaults, shared settings are declared once on the group:

```toml
[conversation.tools.groups.git]
defaults.run = "unattended"
defaults.style.inline_results = "off"
```

Tools in the `git` group inherit `run = "unattended"` and `style.inline_results
= "off"`, and any member tool can still override individual fields in its own
config.

## Design

### `defaults` Section

Groups gain an optional `defaults` section nested under the group definition:

```toml
[conversation.tools.groups.write]
exhaustive = true
defaults.run = "ask"

[conversation.tools.groups.git]
defaults.run = "unattended"
defaults.style.inline_results = "off"
defaults.style.results_file_link = "off"

[conversation.tools.groups.read]
# no defaults — membership-only group
```

The `defaults` namespace prevents collisions between group-level fields (like
`exhaustive`) and tool config fields (like `run`).

### Supported Fields

The `defaults` section accepts the behavioral subset of tool config fields:

| Field       | Type                               | Description                 |
| ----------- | ---------------------------------- | --------------------------- |
| `enable`    | `Enable`                           | Whether the tool is enabled |
| `run`       | `RunMode`                          | How to run the tool         |
| `result`    | `ResultMode`                       | How to deliver tool results |
| `style`     | `DisplayStyleConfig`               | Terminal display settings   |
| `questions` | `IndexMap<String, QuestionConfig>` | Question routing defaults   |

Per-tool identity fields (`source`, `command`, `summary`, `description`,
`examples`, `parameters`, and `options`) are excluded.

The struct reuses the same types as `ToolsDefaultsConfig` (with `questions`
added), so existing serialization, assignment, and delta logic applies.

### Merge Chain

Group defaults sit between `*` defaults and tool-level config in the merge
chain:

```txt
* defaults > group[0].defaults > group[1].defaults > ... > tool config > CLI flags
```

`group[0]`, `group[1]`, and so on are the **included** groups from the tool's
effective `groups` array, in declaration order.
Excluded groups (`!write`) do not contribute defaults.

Following the config system's last-write-wins rule, later layers override
earlier layers, and tool config overrides all group defaults.

**Example:**

```toml
[conversation.tools.groups.write]
exhaustive = true
defaults.run = "ask"

[conversation.tools.groups.verbose]
defaults.style.inline_results = "full"

[conversation.tools.'*']
groups = ["write"]

[conversation.tools.fs_modify_file]
groups = ["write", "verbose"]
# Chain: * > write.defaults > verbose.defaults > tool config
# run = "ask" (from write), inline_results = "full" (from verbose)
# Tool can override either field in its own config.

[conversation.tools.fs_read_file]
groups = ["!write", "read"]
# Chain: * > read.defaults > tool config
# write.defaults does NOT apply (excluded).
```

### Resolution in `ToolConfigWithDefaults`

`ToolConfigWithDefaults` resolves fields by lazy fallback:

```rust
pub fn run(&self) -> RunMode {
    self.tool.run.unwrap_or(self.defaults.run)
}
```

Group defaults add a layer between `tool` and `defaults`.
Each accessor walks `tool > groups > defaults` and returns the first value set:

```rust
pub fn run(&self) -> RunMode {
    self.tool.run
        .or_else(|| self.groups.iter().rev().find_map(|g| g.run))
        .unwrap_or(self.defaults.run)
}
```

`ToolConfigWithDefaults` gains a `groups` field storing the included groups'
defaults in declaration order:

```rust
pub struct ToolConfigWithDefaults {
    /// The tool configuration (unmodified).
    tool: ToolConfig,

    /// Included group defaults, in declaration order.
    groups: Vec<ToolGroupDefaults>,

    /// The global defaults.
    defaults: ToolsDefaultsConfig,
}
```

The constructor stores its inputs without mutating `tool`, so `tool` still
reflects only what the tool itself declared.
This keeps provenance tracing straightforward: compare `tool.run`,
`groups[n].run`, and `defaults.run`.

### Interaction with `*` Defaults

`*` defaults, group defaults, and tool config form three tiers:

- `*` provides the baseline for all tools.
- Group defaults provide a baseline for member tools.
- Tool config provides per-tool settings.

A tool in the `git` group resolves `run` as `tool.run > git.defaults.run >
*.run`, falling back to `*.run` when neither the tool nor `git` sets `run`.

## Drawbacks

**Deeper merge chain.** The chain grows from `* > tool` to `* > groups > tool`,
with each included group adding a layer.
Most tools belong to one or two groups, but tracing where a resolved value came
from is harder until `jp config show` supports merge-chain tracing.

## Alternatives

### Eager merge at construction time

Instead of lazy resolution in accessors, the constructor could clone the tool
config and fold group defaults into it, so accessors stay as
`self.tool.run.unwrap_or(self.defaults.run)`.
This was rejected because mutating `tool` erases whether a value came from the
tool or from a group, unless the struct also stores an unmerged copy.

### Reuse `ToolsDefaultsConfig` directly

Using `ToolsDefaultsConfig` as the group defaults struct was rejected because
its fields are required (for example, `#[setting(required)] pub run: RunMode`).
Global defaults must provide a `run` mode, whereas a group may configure only
`style` and leave `run` unset, so all fields on group defaults must be optional.

## Non-Goals

- **Group overrides.** This RFD covers `defaults` only, which tool-level config
  can override.
  A separate `overrides` section where group config takes precedence over
  tool-level config (for persona-level policy enforcement) is left to a future
  RFD.
- **`jp config show` tracing.** Showing which layer set each resolved value aids
  debugging but is out of scope here.

## Implementation Plan

Depends on [RFD 055] (tool groups with membership and exhaustive validation).

### Phase 1: Group defaults type

1. Add `ToolGroupDefaults` struct with all-optional fields: `enable`, `run`,
   `result`, `style`, `questions`.
   Implement `AssignKeyValue`, `PartialConfigDelta`, and `ToPartial`.
2. Add `defaults: ToolGroupDefaults` to `ToolGroupConfig`.

### Phase 2: Lazy resolution in `ToolConfigWithDefaults`

1. Add `groups: Vec<ToolGroupDefaults>` field to `ToolConfigWithDefaults`.
2. Update `ToolsConfig::get()` and `ToolsConfig::iter()` to look up the tool's
   included groups and pass their defaults when constructing
   `ToolConfigWithDefaults`.
3. Update accessor methods (`run()`, `result()`, `style()`, `is_enabled()`,
   `effective_enable()`, `questions()`) to walk `tool → groups → defaults`.
   The `enable` field resolves per-field (`state` / `allow_toggle`) from the
   stored `EnableConfig` across the same chain (see RFD 081).

## References

- [RFD 055]: tool groups (membership, exhaustive validation, CLI integration)
- [Configuration architecture]: progressive complexity design principles
- `crates/jp_config/src/conversation/tool.rs`: `ToolConfig`,
  `ToolConfigWithDefaults`, `ToolsDefaultsConfig`

[Configuration architecture]: ../configuration.md
[RFD 055]: 055-tool-groups.md
[RFD 057]: 057-group-configuration-overrides.md
