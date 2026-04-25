# RFD D06: Self-Describing Local Tools

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-12

## Summary

This RFD introduces a protocol extension that allows local tool binaries to
describe their own capabilities — name, description, and parameter schema — so
that JP can resolve tool definitions without requiring manual TOML parameter
definitions. When a local tool is configured with `command` but no `parameters`,
JP queries the binary for its schema at resolution time.

## Motivation

Today, registering a local tool requires authoring a TOML config file that
manually specifies every parameter's type, description, and required status. This
is tedious for tool authors and error-prone: the TOML schema and the Rust
implementation can drift apart with no compile-time check.

MCP tools don't have this problem — JP fetches their schema from the MCP server
at runtime and merges user overrides on top. Local tools deserve the same
treatment.

The goal is to make the minimal tool registration look like this:

```toml
[conversation.tools.my_tool]
source = "local"
command = "my-tool-binary"
```

No `parameters`, no `summary`, no `description`. JP gets all of that from the
binary itself. Users can still override any field in TOML, just like they can for
MCP tools.

## Design

### Protocol Extension: `Action::Schema`

The existing `jp_tool::Action` enum has two variants: `Run` and
`FormatArguments`. This RFD adds a third:

```rust
pub enum Action {
    Run,
    FormatArguments,
    Schema,
}
```

When JP needs the schema for a local tool and none is provided in config, it
invokes the tool binary with a `Context` whose `action` field is `"schema"`.
The binary responds with a JSON object describing its tools.

### Schema Response Format

The binary writes a JSON object to stdout:

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

The `tools` array supports binaries that expose multiple tools (like the
existing `.config/jp/tools` binary does). Each entry maps directly to JP's
existing `ToolParameterConfig` shape — the same JSON Schema-like format used in
TOML configs and MCP tool resolution.

Fields follow the same semantics as the TOML config: `summary` is the short
description sent to the LLM, `description` is the detailed text loaded on
demand via `describe_tools`. `required` is inferred from whether a parameter is
present without a `default`.

### Resolution Flow

When `resolve_tool` encounters a `ToolSource::Local` tool:

1. Check if the user provided `parameters` in TOML config.
2. If parameters are present, use the existing path (config-only resolution).
3. If no parameters are defined, invoke the binary with `Action::Schema`.
4. Parse the response and build `ToolDefinition` + `ToolDocs`.
5. Merge user-provided overrides (summary, description, examples, parameter
   overrides) on top, using the same merge logic that `resolve_mcp_tool`
   already implements.

This is the same pattern as MCP tool resolution: binary provides the base
schema, user config provides overrides.

### Caching

Schema responses are cached for the lifetime of the tool resolution (per JP
session). The binary is not re-invoked on every tool call — only at tool
definition time.

### Prerequisite: Move `Tool` to `jp_tool`

The `Tool` struct currently lives in `.config/jp/tools/src/lib.rs`, which is a
workspace-specific crate, not part of JP itself. It needs to move to `jp_tool`
because it represents the shared protocol contract between JP and any tool
binary. The `Action` enum and `Context` type already live there.

## Drawbacks

- Adds a subprocess invocation at tool resolution time. For a handful of tools
  this is negligible; for a workspace with many local tools it could add
  noticeable startup latency. Caching mitigates this.
- Binaries that don't understand `Action::Schema` will fail. The resolution
  code needs a fallback path: if the binary exits non-zero or returns
  unparseable output for a schema request, JP should error clearly and tell the
  user to either add parameters to TOML or update the binary.

## Alternatives

**Custom CLI flag (`--jp-schema`).** Instead of using the existing
`Action`/`Context` protocol, define a CLI convention. Rejected because the
protocol already exists and works — adding a parallel convention creates two
ways to do the same thing.

**Require all local tools to have TOML schemas.** This is the status quo. It
works but creates friction for tool authors, especially when the schema is
already encoded in the tool's Rust types.

## Non-Goals

- **Automatic PATH scanning for `jp-*` binaries.** Discovery and registration
  remain separate concerns. This RFD covers schema acquisition, not discovery.
  The `jp-*` naming convention may be documented as a recommendation for
  third-party tool authors but is not part of this design.
- **Typed Rust SDK for tool authoring.** How tool authors *produce* the schema
  response (e.g. using schemars derive) is covered in a separate RFD.

## Risks and Open Questions

- **Multi-tool binaries.** The current `.config/jp/tools` binary serves many
  tools from one binary. The `tools` array in the schema response handles this,
  but the resolution logic needs to match schema entries to TOML config entries
  by name. Need to verify this works cleanly with the existing `ToolSource::Local
  { tool }` field.
- **Error reporting.** If a binary doesn't support `Action::Schema`, the error
  message needs to clearly guide the user toward either adding parameters in
  TOML or updating the binary. Silent failures would be confusing.

## Implementation Plan

### Phase 1: Move `Tool` to `jp_tool`

Move the `Tool` struct and related types from `.config/jp/tools/src/lib.rs` to
`jp_tool`. Update imports in the workspace tools crate. No behavioral change.

### Phase 2: Add `Action::Schema`

Add the `Schema` variant to `jp_tool::Action`. Define the schema response
format. Update the workspace tools binary to handle the new action by returning
schemas for all its tools.

### Phase 3: Auto-schema resolution in `jp_llm`

Update `resolve_tool` (in `crates/jp_llm/src/tool.rs`) to detect local tools
without parameters and invoke the schema action. Reuse the merge logic from
`resolve_mcp_tool`.

## References

- `crates/jp_tool/src/lib.rs` — current `Action` enum and `Context` type
- `crates/jp_llm/src/tool.rs` — `resolve_tool` and `resolve_mcp_tool` functions
- `.config/jp/tools/src/lib.rs` — `Tool` struct to be migrated
