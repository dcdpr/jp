# RFD 057: Group Configuration Overrides

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-20
- **Extends**: [RFD 056]
- **Summary**: Adds group `overrides` section to enforce tool configuration that
  outlasts tool-level settings but yields to CLI flags.

## Summary

This RFD adds an `overrides` section to tool group definitions.
Unlike `defaults`, which tool-level config can override, `overrides` take
priority over tool-level settings and yield only to CLI flags.

## Motivation

[RFD 056] adds group defaults: fallback configuration that individual tools can
override to reduce repetition.
However, group defaults cannot support **group-level config enforcement** when a
tool already defines its own setting.

Consider a workspace config that sets `run` on individual tools, paired with a
config file loaded via `--cfg` that must enforce a policy across all write
tools:

```toml
# Workspace config: tools set their own run mode
[conversation.tools.fs_modify_file]
run = "unattended"

[conversation.tools.fs_create_file]
run = "unattended"
```

```toml
# review.toml — loaded via --cfg
# Goal: force all write tools to ask before running, regardless of per-tool settings.
# Problem: group defaults can't do this — tool-level "unattended" overrides them.
```

Setting `defaults.run = "ask"` on the `write` group does not work here because
each tool's `run = "unattended"` overrides the group default.

Group overrides solve this by sitting *after* tool-level config in the merge
chain, so `overrides.run = "ask"` takes priority over per-tool settings:

```toml
# review.toml
[conversation.tools.groups.write]
overrides.run = "ask"
```

## Design

### `overrides` Section

Groups gain an optional `overrides` section alongside `defaults`:

```toml
[conversation.tools.groups.write]
exhaustive = true
defaults.run = "ask" # fallback if tool doesn't set run

[conversation.tools.groups.safety]
overrides.run = "ask" # enforced regardless of tool config
```

Both sections accept the same fields: `enable`, `run`, `result`, `style`, and
`questions` (see [RFD 056]).
A group can define `defaults`, `overrides`, or both:

```toml
[conversation.tools.groups.safety]
defaults.style.inline_results = "full" # tool can customize display
overrides.run = "ask" # tool cannot skip confirmation
```

### Merge Chain

With both defaults and overrides, tool configuration resolves in the following
order (later positions win):

```txt
* > group[0..n].defaults > tool config > group[0..n].overrides > CLI flags
```

Because overrides sit between tool config and CLI flags, they take precedence
over per-tool settings while leaving CLI flags as the final authority.
When a tool belongs to multiple groups, overrides are applied in the order of
the tool's `groups` array, so later groups win, matching `defaults`.

For example:

```toml
[conversation.tools.groups.dev]
overrides.enable = true
overrides.run = "unattended"

[conversation.tools.groups.safety]
overrides.run = "ask"

[conversation.tools.cargo_check]
groups = ["dev"]
run = "unattended"
# Chain: * > dev.defaults > tool (run=unattended) > dev.overrides (run=unattended) > CLI
# Both tool and override agree. Effective run = "unattended".

[conversation.tools.fs_modify_file]
groups = ["dev", "safety"]
run = "unattended"
# Chain: * > dev.defaults > safety.defaults > tool (run=unattended)
#        > dev.overrides (run=unattended) > safety.overrides (run=ask) → CLI
# safety.overrides wins over both tool config and dev.overrides.
# Effective run = "ask".
```

### Force-Enabling via `enable`

A `--cfg` file can use `overrides.enable` to force-enable every tool in a group:

```toml
# dev-tools.toml
[conversation.tools.groups.dev]
overrides.enable = true
```

This overrides per-tool `enable` settings, so a tool with `enable = false` in
the workspace config is enabled when `dev-tools.toml` is loaded.
CLI flags can still disable individual tools:

```sh
# Config file force-enables dev tools, but CLI can override
jp query -c dev-tools.toml -t dev -T fs_modify_file "review this"
```

### `apply_enable_tools` Group Awareness

In `query.rs`, `apply_enable_tools` resolves each tool's effective enable state
through `PartialEnableConfig::effective` (merging per-field over the `*`
defaults; see RFD 081) before applying `--tools` and `--no-tools`.
Because `overrides.enable` on a group can override a tool's own `enable` field,
`apply_enable_tools` must resolve effective enable state through the full chain
(`* > group.defaults > tool > group.overrides`) before running the directive
engine's scope and policy rules.

## Drawbacks

**Five-position merge chain.** `* > group.defaults > tool > group.overrides >
CLI` is the deepest merge chain in the config system, making it harder to trace
where an effective value came from.
The config enforcement use case justifies this complexity, and future `jp config
show` tracing can make value provenance visible.

**Override power.** A `--cfg` file with group overrides can alter tool behavior
without touching per-tool config.
A misconfigured override (such as `overrides.enable = false` on a group of
important tools) can cause subtle issues, though CLI flags always win and leave
final authority with the user.

**Two config sections per group.** Group authors must understand the difference
between `defaults` and `overrides`.
Most groups only need `defaults`, so `overrides` is a power-user feature, but
having both sections adds a design choice whenever a group is defined.

## Alternatives

### Group-level `mode` instead of two sections

Instead of separate `defaults` and `overrides` sections, a group could carry a
single `config` section with a `mode` field:

```toml
[conversation.tools.groups.safety]
mode = "override"
config.run = "ask"
config.style.hidden = false
```

This is all-or-nothing: setting a default for `style` while overriding `run`
requires two separate groups.
Splitting `defaults` and `overrides` gives per-field control without per-field
annotations.

### Per-field priority flags

Each value could carry its own priority annotation:

```toml
[conversation.tools.groups.safety]
config.run = { value = "ask", priority = "override" }
```

This allows per-field priority within a single section, but turns every field
into a tagged union, complicating both the TOML syntax and the implementation.

## Non-Goals

- **`jp config show` tracing.** Showing whether an effective value came from
  `defaults`, tool config, or `overrides` is useful but orthogonal to this RFD.
- **Override-of-override resolution.** When two `--cfg` files define overrides
  for the same group, standard config layer merging applies and the later
  `--cfg` wins.

## Risks and Open Questions

**Interaction with `allow_toggle = "if_named"`.** If a group sets
`overrides.enable` with `allow_toggle = "if_named"`, member tools do not respond
to `--tools GROUP` (per the enable table in [RFD 055]).
This is internally consistent, since `if_named` requires naming the tool
directly, but surprising because the group's own override blocks group
activation.
Documentation should direct users to `allow_toggle = "if_named_or_group"` (RFD
081) when a tool should stay hidden by default while still responding to group
activation.

**CLI vs. group `allow_toggle = "never"`.** Under RFD 081's directive engine, a
group override that sets `allow_toggle = "never"` would block a named CLI
directive, conflicting with the rule that CLI flags override group overrides.
This RFD must decide whether group-sourced `allow_toggle` blocks CLI directives
or whether CLI directives bypass it.

## Implementation Plan

Depends on [RFD 056] (group configuration defaults).

### Phase 1: Overrides type and merge chain

1. Add `overrides: ToolGroupDefaults` to `ToolGroupConfig`, reusing the same
   struct as `defaults`.

2. Add `overrides: Vec<ToolGroupDefaults>` to `ToolConfigWithDefaults` alongside
   the existing `groups` field for defaults.

3. Update `ToolsConfig::get()` and `ToolsConfig::iter()` to pass group overrides
   when constructing `ToolConfigWithDefaults`.

4. Update accessor methods to resolve through the full chain so overrides take
   priority over tool config:

   ```rust
   pub fn run(&self) -> RunMode {
       // Overrides win over tool config
       self.overrides.iter().rev().find_map(|g| g.run)
           // Then tool config
           .or(self.tool.run)
           // Then group defaults
           .or_else(|| self.groups.iter().rev().find_map(|g| g.run))
           // Then * defaults
           .unwrap_or(self.defaults.run)
   }
   ```

### Phase 2: CLI integration

1. Update `apply_enable_tools` to resolve each tool's effective enable through
   the full chain (`* > group.defaults > tool > group.overrides`) before
   applying CLI flags.

## References

- [RFD 055]: tool groups (membership, exhaustive validation, CLI integration)
- [RFD 056]: group configuration defaults
- `crates/jp_cli/src/cmd/query.rs`: `apply_enable_tools`
- `crates/jp_config/src/conversation/tool.rs`: `ToolConfigWithDefaults`

[RFD 055]: 055-tool-groups.md
[RFD 056]: 056-group-configuration-defaults.md
