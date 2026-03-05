# RFD 030: Schema DSL

- **Status**: Implemented
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-05

## Summary

JP accepts a concise DSL for defining JSON Schema objects via `--schema` /
`-s`. This guide documents the syntax, rules, and examples for the DSL.

The DSL is designed for the common case: flat or moderately nested objects with
typed fields and descriptions. For complex schemas, full JSON Schema is
accepted as a passthrough.

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

A field has four parts, all except the name are optional:

```
[?] name [type] [: description]
```

### Name

Any sequence of non-reserved characters. Reserved characters are:

```
space  tab  newline  ,  :  [  ]  {  }  |  ?  \  "
```

Names containing reserved characters (or spaces) can be quoted:

```
"my field" int
"items[0]" string
```

### Optional marker

Prefix a field with `?` to exclude it from the `required` array:

```
name, ?nickname, ?age int
```

Produces:

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

If all fields are optional, the `required` key is omitted entirely.

### Default type

When no type is specified, fields default to `string`.

## Types

### Primitives

| DSL | JSON Schema `type` |
|-----|--------------------|
| `str`, `string` | `"string"` |
| `int`, `integer` | `"integer"` |
| `float`, `number` | `"number"` |
| `bool`, `boolean` | `"boolean"` |
| `any` | `{}` (no type constraint) |

A word following a field name is always interpreted as a type. If it is not a
recognized keyword or literal, the parser produces an error:

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

Arrays can contain union items (see below) or nested objects:

```
people [{ name, ?age int }]
```

### Objects

Curly braces define nested object types:

```
address { city, zip, ?state }
```

Objects must contain at least one field — empty objects (`{}`) are rejected
because LLM providers in strict mode require explicit property definitions.

Objects can be nested arbitrarily:

```
a { b { c } }
```

### Literal Values

Quoted strings, numbers, `true`, `false`, and `null` in the type position
define literal (constant) values:

```
kind "fixed"                   → {"const": "fixed"}
version 1                      → {"const": 1}
ratio 0.5                      → {"const": 0.5}
answer true                    → {"const": true}
cleared null                   → {"const": null}
```

Strings must be quoted in the type position to distinguish them from type
keywords. `string` is the type; `"string"` is the literal value `"string"`.

### Unions

The pipe `|` creates union types. When all variants are literals, the output
uses `enum` (widely supported by LLM providers in strict mode). When the
union mixes literals and types, the output uses `anyOf`.

**All literals — `enum`:**

```
status "active"|"inactive"|"archived"
```

```json
{"enum": ["active", "inactive", "archived"]}
```

**Mixed types — `anyOf`:**

```
value "special"|int
```

```json
{"anyOf": [{"const": "special"}, {"type": "integer"}]}
```

**Mixed literals (different JSON types) — `enum`:**

```
value "foo"|"bar"|42
```

```json
{"enum": ["foo", "bar", 42]}
```

Inside arrays, `|` defines which item types the array accepts:

```
data [string|int]
```

```json
{"type": "array", "items": {"anyOf": [{"type": "string"}, {"type": "integer"}]}}
```

Arrays with literal items:

```
tags ["foo"|"bar"|"baz"]
```

```json
{"type": "array", "items": {"enum": ["foo", "bar", "baz"]}}
```

At the field level, `|` creates a union of the entire type:

```
value [string]|int
```

```json
{"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "integer"}]}
```

## Descriptions

A colon after the type (or name, if no type) starts a description. Descriptions
are passed as the `description` field in JSON Schema, which models use as a
generation hint.

### Inline

The simplest form. Ends at the next comma, newline, or closing `}`:

```
summary: a brief two-sentence summary
```

### Quoted

Use double quotes when the description contains commas:

```
bar bool: "hello, universe"
```

### Heredoc

Use triple quotes for multi-line descriptions:

```
baz: """
A longer description that spans
multiple lines.
"""
```

The opening `"""` may be followed by a newline (which is stripped). The closing
`"""` must appear on its own. Internal newlines are preserved.

### On nested types

Descriptions can follow object and array types:

```
address { city, zip }: the mailing address
people [{ name, age int }]: list of people mentioned
```

## Separators

Fields are separated by commas or newlines (or both — a comma followed by a
newline counts as one separator). Trailing separators are allowed.

```
name, age int, active bool      ← commas
name                            ← newlines
age int
active bool
name,                           ← trailing comma
age int,
```

Commas and newlines are interchangeable. Inside `{ }` objects, the same rules
apply.

## Line Continuation

A backslash before a newline joins the current line with the next, allowing a
single field definition to span multiple lines:

```
?age \
      int
```

This also works inside inline descriptions:

```
summary: a long \
  description here
```

The backslash, any trailing whitespace before it, the newline, and any leading
whitespace on the next line are replaced by a single space.

## JSON Passthrough

If the input starts with `{` and parses as valid JSON, it is returned as-is
without DSL processing. This allows full JSON Schema to be used when the DSL
is insufficient:

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
        "role": {"enum": ["engineer", "manager", "designer"]},
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

- [RFD 029: Scriptable Structured Output](029-scriptable-structured-output.md)
  — motivation and design context for the schema DSL
- [JSON Schema specification](https://json-schema.org/)
- [simonw/llm schema DSL](https://llm.datasette.io/en/stable/schemas.html#concise-llm-schema-syntax)
  — prior art that inspired this syntax
- Implementation: `crates/jp_cli/src/schema.rs`
