# RFD 042: Tool Options

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-09
- **Summary**: Adds a per-tool `options` configuration map for passing static
  user-defined settings to tools at runtime.

## Summary

This RFD introduces a per-tool `options` configuration field that passes static,
user-defined key-value pairs to tools at execution time.
This lets users configure tool behavior without polluting the LLM-facing
parameter schema.

## Motivation

Tools receive three categories of input:

| Category    | Source            | Purpose                                |
| ----------- | ----------------- | -------------------------------------- |
| `arguments` | LLM               | What to do (file path, patterns, etc.) |
| `answers`   | Inquiries/prompts | Interactive decisions (apply changes?, |
|             |                   | overwrite?)                            |
| `context`   | Runtime           | Execution environment (root path,      |
|             |                   | action)                                |

None of these let the **user** configure a tool's runtime behavior.
For example, `fs_modify_file` sends an `apply_changes` inquiry to the LLM for
every modification, regardless of how trivial the edit is.
To auto-approve small changes, a user must either set
`questions.apply_changes.answer = true` (which bypasses verification for *all*
changes, including risky ones) or accept an LLM roundtrip for every edit.

Tools need a fourth input category: static behavioral configuration that the
user sets in config and the tool reads at runtime.
This is distinct from `parameters` (which the LLM supplies in a tool call) and
`questions` (which govern inquiry routing).

## Design

### User-Facing Configuration

Per-tool configuration gains an `options` field: a free-form key-value map where
each tool defines its own schema.

```toml
[conversation.tools.fs_modify_file]
options.apply_changes_trigger = "heuristics"
options.auto_approve_max_changed_lines = 10
options.auto_approve_max_ratio_percent = 20
```

Options are per-tool only.
The defaults (`*`) section has no global `options` field because options are
tool-specific and a shared namespace would be ambiguous.

### Config Layer (`jp_config`)

Add `options` to `ToolConfig`:

```rust
/// Per-tool options.
///
/// A free-form map of key-value pairs passed to the tool at runtime.
/// Each tool defines its own supported options and defaults. Unknown
/// options are silently forwarded (the tool ignores what it doesn't
/// recognize).
#[serde(default)]
pub options: Map<String, Value>,
```

Expose `options` through `ToolConfigWithDefaults`:

```rust
impl ToolConfigWithDefaults {
    /// Return the per-tool options map.
    #[must_use]
    pub fn options(&self) -> &Map<String, Value> {
        &self.tool.options
    }
}
```

The `options` field implements the standard config traits: `AssignKeyValue`,
`PartialConfigDelta`, and `ToPartial`.
Because the value is a flat `Map<String, Value>`, delta compares entries by key
and partial serializes non-empty maps.

### Execution Layer (`jp_llm`)

In `execute_local`, include `options` in the JSON context passed to the tool
command:

```rust
let ctx = json!({
    "tool": {
        "name": name,
        "arguments": &arguments,
        "answers": answers,
        "options": config.options(),
    },
    "context": {
        "action": Action::Run,
        "root": root.as_str(),
    },
});
```

The same addition applies to the `FormatArguments` path in
`crates/jp_cli/src/cmd/query/tool/renderer.rs`.

### Other Tool Sources

Options apply only to **local** tools.
For **MCP** tools, the server owns behavior configuration; JP cannot pass
out-of-band options to an external server.
For **builtin** tools, the execution path calls `BuiltinTool::execute(arguments,
answers)` directly; if a builtin tool needs configurable behavior later, the
`BuiltinTool` trait can be extended separately.

## Drawbacks

**No schema validation.** Because `options` is a `Map<String, Value>`, neither
compile-time nor config-time validation checks whether a tool supports a given
key or expects a given value type.
Typos are silently ignored.
Unlike `arguments`, which have a JSON schema for the LLM to follow, `options`
are validated only by the tool itself.

**Discovery.** Without an introspection mechanism, users must read a tool's
documentation or source code to see what options it supports.
This is acceptable while JP's tool set is small and documented, but may need
revisiting if the option surface grows.

## Alternatives

### Typed Per-Tool Config Sections

Instead of a free-form map, define a typed struct per tool in `jp_config`:

```rust
pub struct FsModifyFileOptions {
    pub apply_changes_trigger: ApplyChangesTrigger,
    pub auto_approve_max_changed_lines: usize,
    // ...
}
```

This provides compile-time safety and schema generation, but requires
`jp_config` to know about every tool's options.
It couples the config crate to tool internals and does not scale to user-defined
or MCP tools.

### Overload `questions` Config

Encode behavioral options as question answers (e.g.,
`questions.apply_changes_trigger.answer = "heuristics"`).
This works with existing infrastructure, but abuses the question system for
settings that are not interactive prompts and conflates two distinct concepts in
the config.

### Overload `parameters`

Add hidden parameters that the tool reads but the LLM does not see.
This breaks the contract that `parameters` define the LLM's interface to the
tool: the LLM might hallucinate values for hidden parameters, or validation
might reject tool calls that omit them.

## Non-Goals

- **Global options.** The `*` defaults section does not have an `options` field
  because options are tool-specific.
- **Moving `Tool` to `jp_tool`.** Formalizing the `Tool` struct as a shared
  contract between `jp_llm` and the tools binary is orthogonal and can be done
  as a follow-up without affecting `options`.
- **Options for MCP or builtin tools.** MCP tools are configured server-side,
  and builtin tools can be extended independently if needed.

## Risks and Open Questions

**Threshold for documenting options.** If tools accumulate many options,
discoverability will degrade.
A future extension could add an `options_schema` field to tool definitions
(similar to `parameters`) that declares supported options with their types and
defaults.

## Implementation Plan

1. Add `options: Map<String, Value>` to `ToolConfig` in `jp_config`.
2. Implement `AssignKeyValue`, `PartialConfigDelta`, and `ToPartial` for the new
   field.
3. Add an `options()` accessor to `ToolConfigWithDefaults`.
4. Include `"options"` in the JSON context in `execute_local` and the
   `FormatArguments` path.

This phase can be merged independently with no behavioral change; tools that do
not read `options` are unaffected.

## References

- `crates/jp_config/src/conversation/tool.rs`: `ToolConfig`,
  `ToolConfigWithDefaults`
- `crates/jp_llm/src/tool.rs`: `execute_local`, JSON context construction
