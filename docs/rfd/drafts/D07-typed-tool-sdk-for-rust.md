# RFD D07: Typed Tool SDK for Rust

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-12
- **Requires**: [RFD D06]
- **Summary**: Extends `jp_tool` with `schemars`-based schema generation and
  typed argument deserialization for Rust local tools.

## Summary

This RFD extends `jp_tool` into a Rust tool-authoring SDK backed by `schemars`.
Tool authors define arguments as Rust structs with `#[derive(Deserialize,
JsonSchema)]`, and `jp_tool` generates the tool's parameter schema and
deserializes typed arguments directly.

## Motivation

Writing a JP tool in Rust requires maintaining the parameter schema in two
places:

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

Renaming a parameter in one place without updating the other causes a runtime
error.
Parameter types, descriptions, and whether each parameter is required are
specified twice with no compile-time check between them.

MCP tools and `rmcp` (the official Rust MCP SDK) solve this with `schemars`:
argument types derive `JsonSchema`, doc comments become parameter descriptions,
and `Option<T>` marks optional parameters.
The Rust type definition is the schema.

JP should provide the same workflow for local Rust tools.

## Design

### Schema Generation with `schemars`

`jp_tool` depends on `schemars`.
Tool authors derive `Deserialize` and `JsonSchema` on their argument structs:

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

From `CargoCheckArgs`, `schemars` generates a JSON Schema:

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

`jp_tool::schema_for_args` converts that JSON Schema into JP's parameter schema
format, as defined by the `Action::Schema` protocol in [RFD D06]:

```rust
/// Convert a schemars-generated JSON Schema for type T into JP's
/// tool parameter schema format.
pub fn schema_for_args<T: JsonSchema>() -> Vec<(String, ToolParameterSchema)> {
    // Generate schema via schemars, walk properties, convert to JP format
}
```

`schema_for_args` extracts `properties`, `required`, `description`, and
`default` from the JSON Schema and maps them to the fields used in JP's schema
response and `ToolParameterConfig`.

### Typed Argument Deserialization

Tool functions receive a deserialized struct instead of extracting arguments
from a `Map<String, Value>` with `t.req()` and `t.opt()`:

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

Each tool module's `run()` dispatcher deserializes the `Tool` arguments map into
the typed struct using `serde_json::from_value`.

### Doc Comment Mapping

`schemars` extracts `///` doc comments into the `description` field of the
generated schema.
JP's tool configuration separates `summary` (sent to the LLM in every request)
from `description` (loaded on demand via `describe_tools`).

In the initial implementation, doc comments map to `summary`.
Authors who need both tiers can provide the detailed description with
`#[schemars(description = "longer text")]`.

### `jp_tool` API Surface

With these additions, `jp_tool` provides:

1. **Protocol types** (existing): `Context`, `Action`, `Outcome`, and
   `Question`.
2. **Tool call types** (from [RFD D06]): the `Tool` struct, migrated from
   workspace tools.
3. **Schema generation** (new): `schema_for_args::<T>()` and a re-exported
   `schemars`.
4. **Schema response builder** (new): helpers to build the `Action::Schema` JSON
   response from typed argument structs.

### Deferred: Proc Macro

A `#[jp_tool]` proc macro could generate the dispatch boilerplate for
`Action::Schema` and `Action::Run`, deserialize arguments, and route calls by
tool name.
This RFD defers a macro in favor of library functions until boilerplate across
tools justifies maintaining a proc macro.

The `rmcp` crate's `#[tool]` and `#[tool_router]` macros validate this pattern,
but they are coupled to MCP's `ServerHandler` trait.
JP will need its own macro targeting the `jp_tool` protocol.

## Drawbacks

- **New dependency**: `schemars` is well maintained (~920 GitHub stars) and
  compatible with `serde`, but it increases compile time and binary size for
  tool binaries.
  Tool authors who do not want the dependency can continue writing schemas in
  TOML.
- **Schema fidelity gap**: `schemars` generates standard JSON Schema (draft
  2020-12), while JP's `ToolParameterConfig` has JP-specific fields (`summary`,
  `description`, and `examples`).
  `schema_for_args` bridges the two formats, but fields such as `examples`
  cannot be expressed with `schemars` attributes alone.
- **Migration effort**: Existing workspace tools must be updated to use typed
  argument structs.
  Because `t.req()` and `t.opt()` remain available, tools can migrate
  incrementally.

## Alternatives

- **Custom `#[derive(ToolSchema)]` macro in `jp_macro`**: Generating
  `ToolParameterConfig` directly with JP-specific attributes would give full
  control over the output format, but it requires maintaining a custom proc
  macro that duplicates `schemars`.
  `schemars` covers most of the schema generation work, and a thin conversion
  layer bridges the remaining JP-specific fields (`summary` vs `description`,
  and `examples`).
- **Use `rmcp`'s `#[tool]` macro directly**: `rmcp` already provides the macro
  ergonomics we want, but it is coupled to MCP's `ServerHandler` trait and pulls
  in the full MCP SDK (`tokio`, `tower`, `hyper`, and transport layers).
  JP tools are standalone binaries rather than MCP servers, so the dependency
  cost is not justified.

## Non-Goals

- **Changing tool registration in config**: This RFD covers how tool authors
  implement tools in Rust.
  TOML config, tool sources, and the resolution pipeline are unaffected beyond
  using the `Action::Schema` protocol from [RFD D06].
- **Supporting non-Rust tool authoring**: The typed SDK is specific to Rust.
  Tools in other languages can implement the `Action::Schema` protocol directly
  by returning the expected JSON.

## Risks and Open Questions

- **`schemars` version**: `schemars` v1.0 introduces breaking changes from v0.8.
  We need to evaluate which version to target and check for conflicts with
  existing workspace dependencies.
- **Nested types**: `schemars` generates recursive schemas for nested structs,
  enums, and generics, which `schema_for_args` must map onto the nested
  `properties` and `items` fields of `ToolParameterConfig`.
  We need to verify that this conversion covers every parameter shape used by
  existing tools.
- **Incremental migration**: The `t.req()` and `t.opt()` helpers must remain
  functional alongside typed argument structs so existing tools can migrate one
  at a time.

## Implementation Plan

### Phase 1: Add `schemars` to `jp_tool`

Add `schemars` as an optional feature of `jp_tool`, implement
`schema_for_args::<T>()` to convert JSON Schema into JP's parameter format, and
write tests against existing tool parameter shapes.

### Phase 2: Schema Response Builder

Add helpers that construct the `Action::Schema` JSON response from one or more
typed argument structs, connecting the SDK to the protocol defined in [RFD D06].

### Phase 3: Migrate Workspace Tools

Incrementally convert existing workspace tools from `t.req()` and `t.opt()` to
typed argument structs, starting with simple tools (such as `cargo_check`) and
working toward complex ones (such as `fs_modify_file`).
Each tool migration is independently mergeable.

## References

- [RFD D06]: Self-Describing Local Tools (defines the `Action::Schema` protocol)
- `crates/jp_tool/src/lib.rs`: current `jp_tool` crate
- [schemars crate]: JSON Schema generation
- [rmcp crate]: MCP SDK with `#[tool]` macro (pattern inspiration, not a
  dependency)

[RFD D06]: D06-self-describing-local-tools.md
[rmcp crate]: https://crates.io/crates/rmcp
[schemars crate]: https://crates.io/crates/schemars
