# Workspace Tools

Tools let the assistant call programs and services while working on a
conversation.
Each tool is configured under `conversation.tools.<name>`, where `<name>` is the
name sent to the model.

Tool configuration follows the normal [configuration loading and ordering
rules].
A later layer can override one field without repeating the rest of the tool
definition.

## Tool Sources

Every tool has a `source`:

| Source                | Definition                                                                      |
| --------------------- | ------------------------------------------------------------------------------- |
| `builtin`             | Implemented inside JP.                                                          |
| `local`               | Runs a configured command. JP configuration defines its parameters.             |
| `mcp.<server>`        | Uses the same-named tool from an MCP server. The server defines its parameters. |
| `mcp.<server>.<tool>` | Uses a differently named tool from an MCP server. This allows a local alias.    |

For example, a local tool starts with a complete definition:

```toml
[conversation.tools.word_count]
source = "local"
command = "word-count {{tool.arguments.path}}"
summary = "Count the words in a file."

[conversation.tools.word_count.parameters.path]
type = "string"
required = true
summary = "File to count."
```

An MCP tool points at a configured server:

```toml
[conversation.tools.create_note]
source = "mcp.notes.create_note"
```

JP fetches the MCP tool definition from the server when resolving the tools for
a query.

## Parameter Schemas

A parameter schema describes one argument the model may send in a tool call.
Local tool definitions need complete parameter schemas.
JP supplies schemas for built-in tools.
MCP tools inherit their schemas from the server and use configuration only for
overrides.

Supported JSON types are:

- `array`
- `boolean`
- `integer`
- `null`
- `number`
- `object`
- `string`

The available settings are:

| Setting       | Meaning                                                                                                            |
| ------------- | ------------------------------------------------------------------------------------------------------------------ |
| `type`        | Accepted JSON type, or an array of accepted types.                                                                 |
| `required`    | Whether the argument must be present.                                                                              |
| `default`     | Default advertised to the model. JP fills omitted local-tool arguments; MCP servers handle omitted MCP arguments.  |
| `enum`        | Complete values accepted by this schema node.                                                                      |
| `summary`     | Short description included in the schema sent to the model.                                                        |
| `description` | Detailed documentation available through `describe_tools`. Used as the schema description when `summary` is unset. |
| `examples`    | Usage examples available through `describe_tools`.                                                                 |
| `items`       | Schema applied to every element of an array.                                                                       |
| `properties`  | Schemas for fields of an object.                                                                                   |

### Arrays

An array must define `items`:

```toml
[conversation.tools.my_tool.parameters.tags]
type = "array"
required = true
summary = "Tags to attach."

[conversation.tools.my_tool.parameters.tags.items]
type = "string"
```

The dotted form is equivalent and is convenient for short schemas:

```toml
[conversation.tools.my_tool.parameters.tags]
type = "array"
items.type = "string"
```

### Objects

Object fields live under `properties` and may contain their own arrays or
objects:

```toml
[conversation.tools.my_tool.parameters.target]
type = "object"
required = true

[conversation.tools.my_tool.parameters.target.properties.path]
type = "string"
required = true

[conversation.tools.my_tool.parameters.target.properties.labels]
type = "array"
items.type = "string"
```

## MCP Parameter Overrides

MCP configuration is an overlay on the server schema.
Fields left unset inherit from the server.

For example, suppose an MCP server defines `tags` as an array of strings.
JP can restrict the allowed tags without repeating either type:

```toml
[conversation.tools.bear_note_create]
source = "mcp.grizzly"

[conversation.tools.bear_note_create.parameters.tags.items]
enum = ["projects/jp", "task", "idea"]
```

The effective schema is:

```json
{
  "type": "array",
  "items": {
    "type": "string",
    "enum": ["projects/jp", "task", "idea"]
  }
}
```

Repeating inherited types is accepted when they match the server:

```toml
[conversation.tools.bear_note_create.parameters.tags]
type = "array"

[conversation.tools.bear_note_create.parameters.tags.items]
type = "string"
enum = ["projects/jp", "task", "idea"]
```

The override rules are:

- `type` may be omitted.
  If set, it must match the MCP schema.
- `required = true` may make an optional server parameter required.
- `required = false` cannot make a server-required parameter optional.
- An unset `enum` inherits the server enum.
- `enum = []` removes an inherited enum.
- `items` and `properties` merge recursively, so a nested override does not
  replace the rest of the inherited shape.

Some MCP schemas use a `$ref` without exposing a directly usable type at that
node.
If JP reports that an item schema does not declare a supported type, set
`items.type` to the matching server type alongside the override.

## Enum Scope

`enum` always constrains the value at the location where it appears.

For a scalar parameter, put it on the parameter:

```toml
[conversation.tools.my_tool.parameters.action]
type = "string"
enum = ["create", "update", "delete"]
```

For array elements, put it under `items`:

```toml
[conversation.tools.my_tool.parameters.tags]
type = "array"
items.type = "string"
items.enum = ["task", "idea"]
```

An enum directly on an array constrains complete arrays.
Every enum value must therefore be an array:

```toml
[conversation.tools.my_tool.parameters.tags]
type = "array"
items.type = "string"
enum = [
    ["task"],
    ["task", "idea"],
]
```

This accepts exactly `["task"]` and `["task", "idea"]`.
It does not mean that `"task"` and `"idea"` are independently valid elements.

A single enum value can force one exact value:

```toml
[conversation.tools.my_tool.parameters.format]
type = "string"
enum = ["json"]
```

## Parameters And Options

Parameters are controlled by the model and appear in tool call arguments.
Options are static user configuration passed to the tool at runtime.

Each tool defines which options it accepts.
For example, a tool that documents a `mode` option could be configured as:

```toml
[conversation.tools.my_tool.options]
mode = "strict"
```

Use `parameters` for the model-facing interface.
Use `options` for behavior that the user selects and the model must not change.

## Schema Validation

JP validates the effective schema after local configuration or MCP overrides
have been resolved and before sending a request to a model provider.
Validation rejects:

- Unsupported or duplicate type names.
- Local and built-in parameters without a type.
- Arrays without an item schema.
- `items` on a non-array type.
- `properties` on a non-object type.
- Duplicate or type-incompatible enum values.
- Defaults that do not satisfy the parameter, item, or property schema.
- MCP type overrides that disagree with the server.

Errors include the tool and parameter path:

```text
Invalid schema at `tools.bear_note_create.parameters.tags.enum`: enum value
"projects/jp" has type string, but the schema requires array; use
`tools.bear_note_create.parameters.tags.items.enum` to constrain array elements
```

This validation happens before the provider request, so a bad schema does not
become an HTTP error from the model provider.

## Compatibility Notes

Existing scalar parameters and complete local schemas keep the same syntax.
Existing MCP overrides that repeat matching types also remain valid.

Configurations need attention when they rely on one of these forms:

- A scalar enum placed directly on an array.
  Move it to `items.enum`.
- An MCP override that changes a server-declared type.
  Match the server type or remove the redundant override.
- An array without `items`, or a local parameter without `type`.
  Complete the schema so JP can validate it locally.
- An array enum containing complete arrays that depended on provider-specific
  flattening.
  Complete-array enums retain their JSON Schema meaning; use `items.enum` to
  constrain individual elements.
- `enum = []` on an MCP parameter.
  This explicitly removes the inherited enum; omit the setting to inherit it.

[configuration loading and ordering rules]: ../configuration.md
