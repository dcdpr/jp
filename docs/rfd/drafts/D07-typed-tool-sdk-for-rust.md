# RFD D07: Typed Tool SDK for Rust

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-12

## Summary

This RFD evolves `jp_tool` from a thin protocol crate into a tool authoring SDK.
Tool authors define arguments as Rust structs with `#[derive(Deserialize,
JsonSchema)]`, and the SDK handles schema generation and typed argument
deserialization — eliminating manual string-keyed extraction and the disconnect
between TOML schemas and implementation code.

## Motivation

Writing a JP tool in Rust today requires maintaining the schema in two
disconnected places:

**TOML config** (what the LLM sees):

```toml
[conversation.tools.cargo_check.parameters.package]
type = "string"
summary = "Package to check."
```

**Rust code** (what the tool executes):

```rust
let package: Option<String> = t.opt("package")?;
```

If the parameter name changes in one place but not the other, the result is a
runtime error. Types, required-ness, and descriptions are all specified twice
with no compile-time link between them.

MCP tools and rmcp (the official Rust MCP SDK) solve this with schemars:
argument types derive `JsonSchema`, and the schema is generated from the type
definition. Doc comments become descriptions. `Option<T>` means optional. The
code *is* the schema.

JP should offer the same experience for local tool authors.

## Design

### Approach: schemars + Conversion Function

Add `schemars` as a dependency of `jp_tool`. Tool authors derive `JsonSchema`
on their argument structs. `jp_tool` provides a function to convert the
schemars-generated JSON Schema into JP's schema response format (as defined in
the `Action::Schema` protocol from RFD D06).

```rust
use serde::Deserialize;
use schemars::JsonSchema;

/// Arguments for the cargo check tool.
#[derive(Deserialize, JsonSchema)]
struct CargoCheckArgs {
    /// Package to run check for.
    package: Option<String>,
}
```

From this struct, schemars generates:

```json
{
  "type": "object",
  "properties": {
    "package": {
      "type": ["string", "null"],
      "description": "Package to run check for."
    }
  }
}
```

`jp_tool` provides a conversion function:

```rust
/// Convert a schemars-generated JSON Schema for type T into JP's
/// tool parameter schema format.
pub fn schema_for_args<T: JsonSchema>() -> Vec<(String, ToolParameterSchema)> {
    // Generate schema via schemars, walk properties, convert to JP format
}
```

This function extracts `properties`, `required`, `description`, and `default`
from the JSON Schema and maps them to the fields JP uses in its schema response
and `ToolParameterConfig`.

### Typed Argument Deserialization

Instead of extracting arguments from a `Map<String, Value>` via `t.req()` /
`t.opt()`, tool functions receive a deserialized struct:

```rust
// Before
pub async fn cargo_check(ctx: &Context, package: Option<String>) -> ToolResult {
    // called via: cargo_check(&ctx, t.opt("package")?)
}

// After
pub async fn cargo_check(ctx: &Context, args: CargoCheckArgs) -> ToolResult {
    // args.package is already Option<String>
}
```

The dispatch layer (`run()` in each tool module) deserializes the `Tool`
arguments map into the typed struct using `serde_json::from_value`.

### Doc Comment Mapping

Schemars extracts `///` doc comments as `description` in the generated schema.
JP's tool system distinguishes `summary` (sent to the LLM in every request)
from `description` (loaded on demand via `describe_tools`).

For the initial implementation, doc comments map to `summary`. Authors who need
the two-tier split can use `#[schemars(description = "longer text")]` for the
detailed description.

### What `jp_tool` Becomes

After this RFD, `jp_tool` provides:

1. **Protocol types** (existing): `Context`, `Action`, `Outcome`, `Question`
2. **Tool call types** (migrated from workspace tools, per RFD D06): `Tool` struct
3. **Schema generation** (new): `schema_for_args::<T>()`, re-exported schemars
4. **Schema response builder** (new): helpers to construct the `Action::Schema`
   response JSON from typed args structs

### Future: Proc Macro

A `#[jp_tool]` proc macro could automate the dispatch boilerplate — handling
`Action::Schema` vs `Action::Run`, deserializing arguments, routing by tool
name. This is explicitly deferred: start with library functions and promote to a
macro when the boilerplate across many tools justifies the complexity.

The rmcp crate's `#[tool]` / `#[tool_router]` macros validate this pattern in
practice but are tightly coupled to MCP's `ServerHandler` trait. JP would need
its own macro targeting JP's protocol.

## Drawbacks

- **New dependency.** schemars adds to compile time and binary size for tool
  binaries. It's a well-maintained crate (~920 stars, serde-compatible), but
  it's still a dependency. Tool authors who don't want it can continue using the
  manual TOML approach.
- **Schema fidelity gap.** schemars generates standard JSON Schema (draft
  2020-12). JP's `ToolParameterConfig` has JP-specific fields (`summary` vs
  `description`, `examples`). The conversion function bridges this, but some
  JP features (like the `examples` field) can't be expressed via schemars
  attributes alone.
- **Migration cost.** Existing workspace tools would need to be updated to use
  typed args structs. This can be done incrementally — the old `t.req()` /
  `t.opt()` API remains available.

## Alternatives

**Custom derive macro instead of schemars.** A `#[derive(ToolSchema)]` in
`jp_macro` that generates `ToolParameterConfig` directly with JP-specific
attributes. More control over the output format, but requires maintaining a
proc macro and duplicates what schemars already does well. schemars is the
pragmatic choice — it gets 90% of the way there, and the 10% gap (summary vs
description, examples) can be bridged with a thin conversion layer.

**Use rmcp's `#[tool]` macro directly.** rmcp already has the macro we'd want,
but it's coupled to the MCP `ServerHandler` trait and pulls in the entire MCP
SDK (tokio, tower, hyper, transport layer). JP tools are simple binaries, not
MCP servers. The dependency cost is not justified.

## Non-Goals

- **Changing how tools are registered in config.** This RFD is about how tool
  authors *implement* tools in Rust. TOML config, tool sources, and the
  resolution pipeline are unaffected (except for leveraging `Action::Schema`
  from RFD D06).
- **Supporting non-Rust tool authoring.** The typed SDK is Rust-specific. Tools
  in other languages can implement the `Action::Schema` protocol directly by
  returning the expected JSON.

## Risks and Open Questions

- **schemars version.** schemars v1.0 was released recently with breaking
  changes from 0.8. Need to evaluate which version to target and whether it
  conflicts with any existing workspace dependencies.
- **Nested types.** schemars handles nested structs, enums, and generics. JP's
  `ToolParameterConfig` supports nested `properties` and `items` for objects
  and arrays. The conversion function needs to handle recursive schema
  structures. Need to verify this works for the existing tool parameter shapes.
- **Incremental migration.** Existing tools should be migratable one at a time.
  The old `t.req()` / `t.opt()` API must remain functional alongside the new
  typed approach.

## Implementation Plan

### Phase 1: Add schemars to `jp_tool`

Add schemars as an optional feature of `jp_tool`. Implement
`schema_for_args::<T>()` and the conversion from JSON Schema to JP's parameter
format. Write tests against the existing tool parameter shapes.

### Phase 2: Schema response builder

Add helpers to construct the `Action::Schema` JSON response from one or more
typed args structs. This connects the SDK to the protocol defined in RFD D06.

### Phase 3: Migrate workspace tools (incremental)

Convert existing workspace tools one at a time from `t.req()` / `t.opt()` to
typed args structs. Start with simple tools (e.g. `cargo_check`) and work
toward complex ones (e.g. `fs_modify_file`). Each migration is independently
mergeable.

## References

- RFD D06: Self-Describing Local Tools (defines the `Action::Schema` protocol)
- `crates/jp_tool/src/lib.rs` — current `jp_tool` crate
- [schemars crate](https://crates.io/crates/schemars) — JSON Schema generation
- [rmcp crate](https://crates.io/crates/rmcp) — MCP SDK with `#[tool]` macro
  (pattern inspiration, not a dependency)
