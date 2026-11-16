# RFD D06: Self-Describing Local Tools

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-12
- **Required by**: [RFD D07]
- **Summary**: Adds an `Action::Schema` protocol action so local tool binaries
  can report their own description and parameter schema.

## Summary

This RFD adds an `Action::Schema` protocol extension that lets local tool
binaries report their own name, description, and parameter schema.
When a local tool is configured with `command` and no `parameters`, JP queries
the binary for its schema at resolution time instead of requiring manual TOML
parameter definitions.

## Motivation

Registering a local tool requires a TOML config that manually specifies every
parameter's type, description, and required status.
Maintaining these definitions by hand is tedious and error-prone because the
TOML schema and the Rust implementation can drift apart with no compile-time
check.

For MCP tools, JP fetches the schema from the MCP server at runtime and merges
user overrides on top.
Applying the same model to local tools reduces minimal tool registration to:

```toml
[conversation.tools.my_tool]
source = "local"
command = "my-tool-binary"
```

JP queries the binary for `parameters`, `summary`, and `description`, while
still allowing users to override any field in TOML.

## Design

### Protocol Extension: `Action::Schema`

`jp_tool::Action` gains a `Schema` variant alongside `Run` and
`FormatArguments`:

```rust
pub enum Action {
    Run,
    FormatArguments,
    Schema,
}
```

When a local tool has no schema configured in TOML, JP invokes the tool binary
with a `Context` whose `action` field is `"schema"`.

### Schema Response Format

The binary writes a JSON object describing its tools to stdout:

```json
{
  "tools": [
    {
      "name": "cargo_check",
      "summary": "Run cargo check for the given package.",
      "description": "Longer description with details...",
      "parameters": {
        "package": {
          "type": "string",
          "summary": "Package to check."
        }
      }
    }
  ]
}
```

The `tools` array supports binaries that expose multiple tools, such as
`.config/jp/tools`.
Each entry maps to `ToolParameterConfig`, the JSON Schema-like format used in
TOML configs and MCP tool resolution.

Fields follow the same semantics as the TOML config: `summary` is the short
description sent to the LLM, `description` is the detailed text loaded on demand
via `describe_tools`, and `required` is inferred when a parameter has no
`default`.

### Resolution Flow

When `resolve_tool` encounters a `ToolSource::Local` tool:

1. If `parameters` are defined in TOML config, resolve the tool from config
   alone.
2. Otherwise, invoke the binary with `Action::Schema`.
3. Parse the response into `ToolDefinition` and `ToolDocs`.
4. Merge user-provided overrides (`summary`, `description`, `examples`, and
   parameter overrides) on top using the same merge logic as `resolve_mcp_tool`.

### Caching

Schema responses are cached for the lifetime of tool resolution (one JP
session), so the binary is invoked only at tool definition time and not on every
tool call.

### Prerequisite: Move `Tool` to `jp_tool`

The `Tool` struct lives in `.config/jp/tools/src/lib.rs`, a workspace-specific
crate outside JP itself.
Because `Tool` defines the shared protocol contract between JP and any tool
binary, it belongs in `jp_tool` alongside `Action` and `Context`.

## Drawbacks

- **Subprocess invocation at resolution time.** Querying binaries is negligible
  for a handful of tools, but can add noticeable startup latency in workspaces
  with many local tools.
  Caching mitigates this across tool calls.
- **Incompatible binaries.** Binaries that do not implement `Action::Schema`
  fail when queried without `parameters` in TOML.
  If a binary exits non-zero or returns unparseable output for a schema request,
  resolution must fail with an error instructing the user to either add
  `parameters` to TOML or update the binary.

## Alternatives

**Custom CLI flag (`--jp-schema`).** Define a CLI flag convention instead of
using the `Action`/`Context` protocol.
This is rejected because the protocol already exists; a parallel CLI convention
would create two mechanisms for the same purpose.

**Require all local tools to have TOML schemas.** Keeping manual TOML schemas
works, but duplicates schema definitions that are already encoded in the tool's
Rust types.

## Non-Goals

- **Automatic PATH scanning for `jp-*` binaries.** This RFD covers schema
  acquisition, leaving tool discovery and registration separate.
  The `jp-*` naming convention may be documented as a recommendation for
  third-party tool authors, but is not part of this design.
- **Typed Rust SDK for tool authoring.** How tool authors produce the schema
  response (for example, using `schemars` derive) is covered in a separate RFD.

## Risks and Open Questions

- **Multi-tool binaries.** The `.config/jp/tools` binary serves multiple tools
  from a single executable.
  While the `tools` array in the schema response supports this, resolution must
  match schema entries to TOML config entries by name using the
  `ToolSource::Local { tool }` field.
- **Error reporting.** When a binary does not support `Action::Schema`, the
  error message must direct the user to either add `parameters` in TOML or
  update the binary, rather than failing silently.

## Implementation Plan

### Phase 1: Move `Tool` to `jp_tool`

Move the `Tool` struct and related types from `.config/jp/tools/src/lib.rs` to
`jp_tool` and update imports in the workspace tools crate, with no behavioral
change.

### Phase 2: Add `Action::Schema`

Add the `Schema` variant to `jp_tool::Action` and define the schema response
format.
Update the workspace tools binary to handle `Action::Schema` by returning
schemas for all of its tools.

### Phase 3: Auto-schema resolution in `jp_llm`

Update `resolve_tool` in `crates/jp_llm/src/tool.rs` to query `Action::Schema`
for local tools without configured `parameters`, reusing the override merge
logic from `resolve_mcp_tool`.

## References

- `crates/jp_tool/src/lib.rs`: `Action` enum and `Context` type.
- `crates/jp_llm/src/tool.rs`: `resolve_tool` and `resolve_mcp_tool` functions.
- `.config/jp/tools/src/lib.rs`: `Tool` struct to move to `jp_tool`.

[RFD D07]: D07-typed-tool-sdk-for-rust.md
