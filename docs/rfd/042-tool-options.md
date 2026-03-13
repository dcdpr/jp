# RFD 042: Tool Options

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-09

## Summary

This RFD introduces a per-tool `options` field in the tool configuration that
passes static, user-defined key-value pairs to tools at execution time. This
gives tools a way to receive behavioral configuration from the user without
polluting the LLM-facing parameter schema.

## Motivation

Tools currently receive three categories of input:

| Category    | Source            | Purpose                                  |
|-------------|-------------------|------------------------------------------|
| `arguments` | LLM               | What to do (file path, patterns, etc.)   |
| `answers`   | Inquiries/prompts | Interactive decisions (apply changes?,   |
|             |                   | overwrite?)                              |
| `context`   | Runtime           | Execution environment (root path,        |
|             |                   | action)                                  |

There is no mechanism for the **user** to configure a tool's runtime behavior.
For example, `fs_modify_file` always sends an `apply_changes` inquiry to the LLM
for every modification, regardless of how trivial the edit is. A user who wants
the tool to auto-approve small changes today has two options: set
`questions.apply_changes.answer = true` (which bypasses verification for *all*
changes, including risky ones) or accept the cost of an LLM roundtrip for every
edit.

What's missing is a fourth input category — static behavioral configuration from
the user that the tool reads at runtime to adjust its behavior. This is distinct
from `parameters` (which the LLM controls) and `questions` (which govern inquiry
routing). Options are set by the user in config, not by the LLM in a tool call.

## Design

### User-Facing Configuration

A new `options` field on per-tool configuration. The field is a free-form
key-value map — each tool defines its own schema.

```toml
[conversation.tools.fs_modify_file]
options.apply_changes_trigger = "heuristics"
options.auto_approve_max_changed_lines = 10
options.auto_approve_max_ratio_percent = 20
```

Options are per-tool only. There is no global `options` in the defaults (`*`)
section — options are inherently tool-specific, and a shared namespace would be
confusing.

### Config Layer — `jp_config`

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

Expose through `ToolConfigWithDefaults`:

```rust
impl ToolConfigWithDefaults {
    /// Return the per-tool options map.
    #[must_use]
    pub fn options(&self) -> &Map<String, Value> {
        &self.tool.options
    }
}
```

The `options` field needs the standard config trait implementations:
`AssignKeyValue`, `PartialConfigDelta`, and `ToPartial`. Because the value is a
flat `Map<String, Value>`, these are straightforward — delta compares entries by
key, partial serializes non-empty maps.

### Execution Layer — `jp_llm`

In `execute_local`, include options in the JSON context passed to the tool
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

### Applicability to Other Tool Sources

Options only apply to **local** tools. For **MCP** tools, the server owns
behavior configuration — JP has no way to pass out-of-band options to an
external server. For **builtin** tools, the execution path calls
`BuiltinTool::execute(arguments, answers)` directly; if a builtin needs
configurable behavior in the future, the `BuiltinTool` trait can be extended
separately.

## Drawbacks

**No schema validation.** Because options are `Map<String, Value>`, there is no
compile-time or config-time validation that a given option key is supported by
the tool or that the value is the right type. Typos are silently ignored. This
is the same tradeoff as `arguments`, but arguments at least have a JSON schema
the LLM follows. For options, the tool itself is the only validator.

**Discovery.** Users won't know what options a tool supports without reading the
tool's documentation or source. There is no introspection mechanism. This is
acceptable for now — JP's tool set is small and documented — but may need
revisiting if the option surface grows.

## Alternatives

### Typed per-tool config sections

Instead of a free-form map, define a typed struct per tool in `jp_config`:

```rust
pub struct FsModifyFileOptions {
    pub apply_changes_trigger: ApplyChangesTrigger,
    pub auto_approve_max_changed_lines: usize,
    // ...
}
```

This gives compile-time safety and schema generation but requires `jp_config` to
know about every tool's options. It couples the config crate to tool internals
and doesn't scale to user-defined or MCP tools. Rejected in favor of the generic
map.

### Overload `questions` config

Encode behavioral options as question answers (e.g.,
`questions.apply_changes_trigger.answer = "heuristics"`). This technically works
with today's infrastructure but abuses the question system — these aren't
questions the tool asks, they're behavioral knobs. It conflates two distinct
concepts and makes the config harder to understand.

### Overload `parameters`

Add hidden parameters that the LLM doesn't see but the tool reads. This breaks
the contract that parameters are the LLM's interface to the tool. The LLM might
hallucinate values for these hidden parameters, or validation might reject tool
calls that omit them.

## Non-Goals

- **Global options.** The `*` defaults section does not get an `options` field.
  Options are tool-specific by definition.
- **Moving `Tool` to `jp_tool`.** Formalizing the `Tool` struct as a shared
  contract between `jp_llm` and the tools binary is worthwhile but orthogonal.
  It can be done as a follow-up without affecting the options feature.
- **Options for MCP or builtin tools.** Out of scope. MCP tools are configured
  server-side. Builtins can be extended independently if needed.

## Risks and Open Questions

**Threshold for documenting options.** If tools start accumulating options,
discoverability becomes a real problem. A future extension could add an
`options_schema` field to tool definitions (similar to `parameters`) that
describes supported options with types and defaults. This is not needed now but
worth keeping in mind.

## Implementation Plan

1. Add `options: Map<String, Value>` to `ToolConfig` in `jp_config`.
2. Implement `AssignKeyValue`, `PartialConfigDelta`, `ToPartial` for the new
   field.
3. Add `options()` accessor to `ToolConfigWithDefaults`.
4. Include `"options"` in the JSON context in `execute_local` and the
   `FormatArguments` path.

This phase can be merged independently. No behavioral change — tools that don't
read options are unaffected.

## References

- `crates/jp_config/src/conversation/tool.rs` — `ToolConfig`,
  `ToolConfigWithDefaults`
- `crates/jp_llm/src/tool.rs` — `execute_local`, JSON context construction
