# RFD 030: Schema DSL

- **Status**: Implemented
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-05
- **Summary**: Documents the concise `--schema` DSL syntax for defining typed
  JSON Schema objects on the command line.

## Summary

JP accepts a concise DSL via `--schema` / `-s` for defining JSON Schema objects.
The DSL targets the common case: flat or moderately nested objects with typed
fields and descriptions.
For complex schemas, `--schema` accepts full JSON Schema as a passthrough.

## Quick Start

```sh
# Single field — returns {"summary": "..."}
jp q -s 'summary' "summarize this document" -a doc.md

# Multiple typed fields
jp q -s 'name, age int, active bool' "invent a person"

# With descriptions to guide the model
jp q -s 'summary: two sentence summary, sentiment: positive/negative/neutral' "analyze this"

# Enum values
jp q -s 'sentiment "positive"|"negative"|"neutral"' "classify this review" -a review.txt

# Nested objects and arrays
jp q -s 'title, authors [{ name, affiliation }]' "extract metadata" -a paper.pdf

# Full JSON Schema also works
jp q -s '{"type":"object","properties":{"x":{"type":"string"}}}' "..."
```

## Grammar

The formal grammar in ABNF notation:

```abnf
schema       = field-list

field-list   = field *(separator field) [separator]
separator    = 1*("," / LF)

field        = ["?"] name [type-expr] [":" description]

name         = quoted-string / 1*name-char
name-char    = <any character except whitespace, comma, colon,
                brackets, braces, pipe, question mark, backslash,
                or double quote>
             ; i.e. not: SP / HTAB / LF / "," / ":" / "[" / "]" /
             ;          "{" / "}" / "|" / "?" / "\" / DQUOTE

type-expr    = base-type *("|" base-type)
base-type    = primitive / array-type / object-type / literal
primitive    = "str" / "string"
             / "int" / "integer"
             / "float" / "number"
             / "bool" / "boolean"
             / "any"
literal      = quoted-string               ; string literal: "foo"
             / number-literal              ; number literal: 42, -1, 3.14
             / "true" / "false"            ; boolean literals
             / "null"                      ; null literal
number-literal = ["-"] 1*DIGIT ["." 1*DIGIT]
array-type   = "[" [type-expr] "]"          ; [] is sugar for [any]
object-type  = "{" field-list "}"           ; must have >= 1 field

description  = heredoc / quoted-string / inline-text
heredoc      = 3DQUOTE [LF] *CHAR 3DQUOTE  ; triple-quoted, multiline
quoted-string= DQUOTE *(CHAR / "\" CHAR) DQUOTE
inline-text  = *<any char except comma, LF, or current terminator>

; Line continuation: "\" at end of line joins with the next line.
; Applies both between tokens and inside inline descriptions.
continuation = "\" *WSP LF
```

## Fields

A field has four parts; only `name` is required:

```
[?] name [type] [: description]
```

### Name

A field name is any sequence of non-reserved characters.
The reserved characters are:

```
space  tab  newline  ,  :  [  ]  {  }  |  ?  \  "
```

Quote names that contain reserved characters:

```
"my field" int
"items[0]" string
```

### Optional Marker

Prefix a field with `?` to exclude it from the `required` array:

```
name, ?nickname, ?age int
```

This produces:

```json
{
  "type": "object",
  "properties": {
    "name": {"type": "string"},
    "nickname": {"type": "string"},
    "age": {"type": "integer"}
  },
  "required": ["name"]
}
```

If all fields are optional, the schema omits the `required` key.

### Default Type

Fields without an explicit type default to `string`.

## Types

### Primitives

| DSL               | JSON Schema `type`        |
| ----------------- | ------------------------- |
| `str`, `string`   | `"string"`                |
| `int`, `integer`  | `"integer"`               |
| `float`, `number` | `"number"`                |
| `bool`, `boolean` | `"boolean"`               |
| `any`             | `{}` (no type constraint) |

The parser interprets any word following a field name as a type and returns an
error if it is not a recognized keyword or literal:

```
age blorp
    ^^^^^ unknown type 'blorp' (expected: str, int, float, bool, any, or a literal value)
```

### Arrays

Square brackets define array types:

```
tags [string]           → array of strings
scores [int]            → array of integers
items [any]             → array of anything
data []                 → same as [any]
```

Arrays can contain unions or nested objects:

```
people [{ name, ?age int }]
```

### Objects

Curly braces define nested object types:

```
address { city, zip, ?state }
```

Objects must contain at least one field.
The parser rejects empty objects (`{}`) because LLM providers in strict mode
require explicit property definitions.

Objects can nest to any depth:

```
a { b { c } }
```

### Literal Values

Quoted strings, numbers, `true`, `false`, and `null` in the type position define
literal (constant) values:

```
kind "fixed"                   → {"type": "string", "const": "fixed"}
version 1                      → {"type": "integer", "const": 1}
ratio 0.5                      → {"type": "number", "const": 0.5}
answer true                    → {"type": "boolean", "const": true}
cleared null                   → {"type": "null", "const": null}
```

A literal emits both `"type"` and `"const"` in the schema.
`{"type": "integer", "const": 1}` and `{"const": 1}` accept the same documents,
but the first also declares the value's type.

Quote strings in the type position to distinguish them from type keywords:
`string` is the type, whereas `"string"` is the literal value `"string"`.

### Unions

The pipe operator (`|`) combines types into a union.
When all variants are literals of the same type, the parser emits a typed `enum`
(mixing integer and float literals widens the type to `number`).
All other unions emit `anyOf`.

**Literals of one type (typed `enum`):**

```
status "active"|"inactive"|"archived"
```

```json
{"type": "string", "enum": ["active", "inactive", "archived"]}
```

**Mixed types (`anyOf`):**

```
value "special"|int
```

```json
{"anyOf": [{"type": "string", "const": "special"}, {"type": "integer"}]}
```

**Mixed literal types (`anyOf`):**

```
value "foo"|"bar"|42
```

```json
{
  "anyOf": [
    {"type": "string", "const": "foo"},
    {"type": "string", "const": "bar"},
    {"type": "integer", "const": 42}
  ]
}
```

Inside brackets, `|` defines a union of array item types:

```
data [string|int]
```

```json
{"type": "array", "items": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}
```

Literal unions inside an array also collapse to a typed `enum`:

```
tags ["foo"|"bar"|"baz"]
```

```json
{"type": "array", "items": {"type": "string", "enum": ["foo", "bar", "baz"]}}
```

At the field level, `|` forms a union of the entire type:

```
value [string]|int
```

```json
{"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "integer"}]}
```

## Descriptions

A colon (`:`) after the type (or after the field name when the type is omitted)
starts a description.
This sets the `description` field in JSON Schema, which models use as a
generation hint.

### Inline

An unquoted description runs until the next comma, newline, or closing `}`:

```
summary: a brief two-sentence summary
```

### Quoted

Wrap the description in double quotes when it contains commas:

```
bar bool: "hello, universe"
```

### Heredoc

Wrap multi-line descriptions in triple quotes:

```
baz: """
A longer description that spans
multiple lines.
"""
```

The parser strips a newline immediately following the opening `"""` and
preserves internal newlines.
The closing `"""` must appear on its own.

### On Nested Types

Descriptions can follow object and array types:

```
address { city, zip }: the mailing address
people [{ name, age int }]: list of people mentioned
```

## Separators

Commas and newlines are interchangeable field separators at the top level and
inside `{ }` objects.
A comma followed by a newline counts as a single separator, and trailing
separators are allowed:

```
name, age int, active bool      ← commas
name                            ← newlines
age int
active bool
name,                           ← trailing comma
age int,
```

## Line Continuation

A backslash (`\`) before a newline joins the current line with the next so a
field definition can span multiple lines:

```
?age \
      int
```

Line continuations also work inside inline descriptions:

```
summary: a long \
  description here
```

The parser replaces the backslash, any trailing whitespace before it, the
newline, and any leading whitespace on the next line with a single space.

## JSON Passthrough

If the input starts with `{` and parses as valid JSON, the parser returns it
as-is without DSL processing.
Pass a full JSON Schema when the DSL is insufficient:

```sh
jp q -s '{"type":"object","properties":{"x":{"type":"string","minLength":1}}}' "..."
```

## Full Example

```
people {
    name
    ?age int
    role "engineer"|"manager"|"designer"
    misc [any]: whatever you want
    ?nested { data [string] }
}: here is the people description,
foo [string]|int, bar bool: "hello, universe",
baz: """
a longer description here
"""
```

This produces:

```json
{
  "type": "object",
  "properties": {
    "people": {
      "type": "object",
      "properties": {
        "name": {"type": "string"},
        "age": {"type": "integer"},
        "role": {"type": "string", "enum": ["engineer", "manager", "designer"]},
        "misc": {"type": "array", "items": {}, "description": "whatever you want"},
        "nested": {
          "type": "object",
          "properties": {
            "data": {"type": "array", "items": {"type": "string"}}
          },
          "required": ["data"]
        }
      },
      "required": ["name", "role", "misc"],
      "description": "here is the people description"
    },
    "foo": {
      "anyOf": [
        {"type": "array", "items": {"type": "string"}},
        {"type": "integer"}
      ]
    },
    "bar": {"type": "boolean", "description": "hello, universe"},
    "baz": {"type": "string", "description": "a longer description here"}
  },
  "required": ["people", "foo", "bar", "baz"]
}
```

## References

- [RFD 029: Scriptable Structured Output][RFD 029]: motivation and design
  context for the schema DSL
- [JSON Schema specification]
- [simonw/llm schema DSL]: prior art that inspired this syntax
- Implementation: `crates/jp_cli/src/schema.rs`

[JSON Schema specification]: https://json-schema.org/
[RFD 029]: 029-scriptable-structured-output.md
[simonw/llm schema DSL]: https://llm.datasette.io/en/stable/schemas.html#concise-llm-schema-syntax
