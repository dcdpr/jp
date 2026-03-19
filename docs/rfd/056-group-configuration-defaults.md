# RFD 056: Group Configuration Defaults

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-20
- **Extends**: [RFD 055](055-tool-groups.md)
- **Extended by**: [RFD 057](057-group-configuration-overrides.md)

## Summary

This RFD adds a `defaults` section to tool group definitions, allowing groups to
carry fallback configuration (`enable`, `run`, `result`, `style`, `questions`)
for their member tools. The tool's `groups` array ordering determines which
group's defaults take priority.

## Motivation

[RFD 055] introduces tool groups for CLI shorthand and exhaustive validation,
but groups carry no configuration — they are purely membership containers.
Configuring shared behavior across a set of tools still requires per-tool
repetition:

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

A `--cfg` file can bundle these together, but the duplication remains — each
tool repeats the same fields. If the group's intended behavior changes, every
tool must be updated.

Group defaults solve this: declare the shared configuration once on the group,
and member tools inherit it automatically.

```toml
[conversation.tools.groups.git]
defaults.run = "unattended"
defaults.style.inline_results = "off"
```

Tools in the `git` group inherit `run = "unattended"` and `style.inline_results
= "off"` without repeating them. A tool can still override any field in its own
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
|-------------|------------------------------------|-----------------------------|
| `enable`    | `Enable`                           | Whether the tool is enabled |
| `run`       | `RunMode`                          | How to run the tool         |
| `result`    | `ResultMode`                       | How to deliver tool results |
| `style`     | `DisplayStyleConfig`               | Terminal display settings   |
| `questions` | `IndexMap<String, QuestionConfig>` | Question routing defaults   |

Fields excluded (per-tool identity, not behavioral): `source`, `command`,
`summary`, `description`, `examples`, `parameters`, `options`.

The struct reuses the same types as `ToolsDefaultsConfig` (with `questions`
added), so existing serialization, assignment, and delta logic applies.

### Merge Chain

Group defaults sit between `*` defaults and tool-level config in the merge
chain:

```txt
* defaults > group[0].defaults > group[1].defaults > ... > tool config > CLI flags
```

Where `group[0]`, `group[1]`, etc. are the **included** groups from the tool's
effective `groups` array, in declaration order. Excluded groups (`!write`) do
not contribute defaults.

This follows the same last-write-wins principle used throughout JP's config
system: later layers override earlier layers, and tool config overrides all
group defaults.

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

`ToolConfigWithDefaults` currently resolves fields via simple fallback:

```rust
pub fn run(&self) -> RunMode {
    self.tool.run.unwrap_or(self.defaults.run)
}
```

Group defaults extend this pattern. The accessor walks `tool > groups >
defaults`, returning the first value found:

```rust
pub fn run(&self) -> RunMode {
    self.tool.run
        .or_else(|| self.groups.iter().rev().find_map(|g| g.run))
        .unwrap_or(self.defaults.run)
}
```

`ToolConfigWithDefaults` gains a `groups` field storing the included groups'
defaults in declaration order. The constructor stores inputs without mutating
them — `tool` always represents what the tool actually declared:

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

This keeps the lazy fallback pattern that already exists for `* defaults` and
extends it with one additional layer. The `tool` field remains a clean
representation of the tool's own config, which makes provenance tracing
straightforward (compare `tool.run` vs `groups[n].run` vs `defaults.run`).

### Interaction with `*` Defaults

Group defaults and `*` defaults are complementary, not competing:

- `*` provides the universal baseline for all tools.
- Group defaults provide a per-group baseline for member tools.
- Tool config provides per-tool specifics.

A tool in the `git` group resolves `run` as: `tool.run > git.defaults.run >
*.run`. If the tool doesn't set `run` and neither does the `git` group, the `*`
default applies.

## Drawbacks

**Deeper merge chain.** The chain grows from `* > tool` to `* > groups > tool`.
For a tool in multiple groups, each group is a layer. In practice most tools
will be in one or two groups, but debugging "where did this value come from?"
becomes harder until `jp config show` gains merge-chain tracing.

## Alternatives

### Eager merge at construction time

Instead of lazy resolution in accessors, the constructor could clone the tool
config and fold group defaults into it, so accessors stay as
`self.tool.run.unwrap_or(self.defaults.run)`. This was rejected because it
mutates the `tool` field — making it impossible to distinguish "the tool set
this" from "a group set this" without storing a separate unmerged copy.

### Reuse `ToolsDefaultsConfig` directly

The group defaults struct could be exactly `ToolsDefaultsConfig`. This was
rejected because `ToolsDefaultsConfig` uses required fields (e.g.,
`#[setting(required)] pub run: RunMode`) that make sense for global defaults
(every config must provide a run mode) but not for group defaults (a group may
only want to set `style`, leaving `run` unspecified). Group defaults need all
fields optional.

## Non-Goals

- **Group overrides.** This RFD covers `defaults` only — configuration that
  tool-level config can override. A future RFD may add an `overrides` section
  where group config takes priority over tool-level config, enabling
  persona-level policy enforcement.
- **`jp config show` tracing.** Displaying which layer set which value is useful
  for debugging group defaults but is orthogonal to this RFD.

## Implementation Plan

Depends on [RFD 055] (tool groups with membership and exhaustive validation).

### Phase 1: Group defaults type

1. Add `ToolGroupDefaults` struct with all-optional fields: `enable`, `run`,
   `result`, `style`, `questions`. Implement `AssignKeyValue`,
   `PartialConfigDelta`, and `ToPartial`.
2. Add `defaults: ToolGroupDefaults` to `ToolGroupConfig`.

### Phase 2: Lazy resolution in `ToolConfigWithDefaults`

1. Add `groups: Vec<ToolGroupDefaults>` field to `ToolConfigWithDefaults`.
2. Update `ToolsConfig::get()` and `ToolsConfig::iter()` to look up the tool's
   included groups and pass their defaults when constructing
   `ToolConfigWithDefaults`.
3. Update accessor methods (`run()`, `result()`, `style()`, `enable()`,
   `enable_mode()`, `questions()`) to walk `tool → groups → defaults`.

## References

- [RFD 055] — tool groups (membership, exhaustive validation, CLI integration)
- [Configuration architecture] — progressive complexity design principles
- `crates/jp_config/src/conversation/tool.rs` — `ToolConfig`,
  `ToolConfigWithDefaults`, `ToolsDefaultsConfig`

[RFD 055]: 055-tool-groups.md
[Configuration architecture]: ../configuration.md
