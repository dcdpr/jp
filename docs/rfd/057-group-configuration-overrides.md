# RFD 057: Group Configuration Overrides

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-20
- **Extends**: [RFD 056](056-group-configuration-defaults.md)

## Summary

This RFD adds an `overrides` section to tool group definitions. Unlike
`defaults` (which tool-level config can override), `overrides` enforces
configuration that takes priority over tool-level settings. Only CLI flags can
override group overrides.

## Motivation

[RFD 056] adds group defaults — fallback config that tools can override. This
covers the common case of reducing repetition. But a different use case remains
unsolved: **group-level config enforcement**.

Consider a workflow where tools are configured per-tool in the workspace config,
and a config file loaded via `--cfg` needs to guarantee certain behavior:

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

With `defaults.run = "ask"` on the write group, each tool's `run = "unattended"`
wins (tool config overrides group defaults). The config cannot enforce its
policy.

Group overrides solve this: they sit *after* tool-level config in the merge
chain, so the config's `run = "ask"` takes priority over per-tool settings.

```toml
# review.toml
[conversation.tools.groups.write]
overrides.run = "ask"
```

## Design

### `overrides` Section

Groups gain an optional `overrides` section, alongside the existing `defaults`:

```toml
[conversation.tools.groups.write]
exhaustive = true
defaults.run = "ask" # fallback if tool doesn't set run

[conversation.tools.groups.safety]
overrides.run = "ask" # enforced regardless of tool config
```

Both sections accept the same fields (see [RFD 056] for the field list):
`enable`, `run`, `result`, `style`, `questions`.

A group can use `defaults`, `overrides`, or both:

```toml
[conversation.tools.groups.safety]
defaults.style.inline_results = "full" # tool can customize display
overrides.run = "ask" # tool cannot skip confirmation
```

### Merge Chain

The full merge chain with both defaults and overrides:

```txt
* > group[0..n].defaults > tool config > group[0..n].overrides > CLI flags
```

Overrides sit between tool config and CLI flags. This means:

- Tool config cannot override them (that's the point).
- CLI flags can still override them (CLI authority is preserved).

Within the overrides position, group ordering follows the tool's `groups` array
— later groups have higher priority, same as defaults.

**Example:**

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

A common use case is a `--cfg` file that force-enables a group of tools:

```toml
# dev-tools.toml
[conversation.tools.groups.dev]
overrides.enable = true
```

This overrides per-tool `enable` settings. A tool with `enable = false` in the
workspace config becomes enabled when the config file is loaded. CLI flags can
still disable it:

```sh
# Config file force-enables dev tools, but CLI can override
jp query -c dev-tools.toml -t dev -T fs_modify_file "review this"
```

### `apply_enable_tools` Group Awareness

The `apply_enable_tools` function in `query.rs` currently inspects
`PartialToolConfig.enable` to determine CLI behavior. With group overrides, a
tool's effective enable may come from a group's `overrides.enable`, not the
tool's own field.

The CLI enable/disable logic must resolve each tool's effective enable through
the full chain (`* > group.defaults > tool > group.overrides`) before applying
`--tools` / `--no-tools` flags.

## Drawbacks

**Five-position merge chain.** `* > group.defaults > tool > group.overrides >
CLI` is the deepest merge chain in the config system. Debugging "where did this
value come from?" requires understanding all five positions. This cost is
justified by the config enforcement use case but should be mitigated by future
`jp config show` tracing.

**Override power.** A `--cfg` file with group overrides can silently change tool
behavior in ways that are hard to trace. A misconfigured override (e.g.,
`overrides.enable = false` on a group containing important tools) could cause
subtle issues. The mitigation is that CLI flags always win, so the user retains
final authority.

**Two config sections per group.** Users must understand the difference between
`defaults` and `overrides`. Most groups will only use `defaults` — the
`overrides` section is a power-user feature. But its existence means users
encounter a design decision when creating groups.

## Alternatives

### Group-level `mode` instead of two sections

Instead of separate `defaults` and `overrides`, a group could carry a single
`config` section with a `mode` field:

```toml
[conversation.tools.groups.safety]
mode = "override"
config.run = "ask"
config.style.hidden = false
```

This was rejected because it's all-or-nothing: if you want `run` to override but
`style` to be a default, you need two separate groups. The
`defaults`/`overrides` split gives per-field control without per-field syntax.

### Per-field priority flags

Each value could carry its own priority annotation:

```toml
[conversation.tools.groups.safety]
config.run = { value = "ask", priority = "override" }
```

Maximum flexibility, but every field becomes a tagged union. The config syntax
gets ugly, the implementation is complex, and the mental model is harder.

## Non-Goals

- **`jp config show` tracing.** Displaying which layer (defaults vs overrides vs
  tool) set which value. Useful but orthogonal.
- **Override-of-override resolution.** If two `--cfg` files both define
  overrides for the same group, normal config layer merging applies (later
  `--cfg` wins). No special conflict resolution is needed.

## Risks and Open Questions

**Interaction with `enable = "explicit"`.** If a group has `overrides.enable =
"explicit"`, the tool won't respond to `--tools GROUP` (per the enable table in
[RFD 055]). This is internally consistent but surprising — the group's own
override prevents group activation. This should be documented: use
`explicit_or_group` if you want the tool to be hidden by default but still
respond to group activation.

## Implementation Plan

Depends on [RFD 056] (group configuration defaults).

### Phase 1: Overrides type and merge chain

1. Add `overrides: ToolGroupDefaults` to `ToolGroupConfig` (reuses the same
   struct as `defaults`).
2. Add `overrides: Vec<ToolGroupDefaults>` to `ToolConfigWithDefaults`
   (alongside the existing `groups` field for defaults).
3. Update `ToolsConfig::get()` and `ToolsConfig::iter()` to pass group overrides
   when constructing `ToolConfigWithDefaults`.
4. Update accessor methods to resolve through the full chain. Overrides take
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
   the full chain (`* → group.defaults → tool → group.overrides`) before
   applying CLI flags.

## References

- [RFD 055] — tool groups (membership, exhaustive validation, CLI integration)
- [RFD 056] — group configuration defaults
- `crates/jp_cli/src/cmd/query.rs` — `apply_enable_tools`
- `crates/jp_config/src/conversation/tool.rs` — `ToolConfigWithDefaults`

[RFD 055]: 055-tool-groups.md
[RFD 056]: 056-group-configuration-defaults.md
