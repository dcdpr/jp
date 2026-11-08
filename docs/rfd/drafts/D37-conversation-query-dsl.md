# RFD D37: Conversation Query DSL

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-16
- **Summary**: Introduces a `--filter EXPR` predicate DSL in a `jp_query` crate
  for querying conversations by metadata, config, and events.

## Summary

This RFD introduces a predicate language for querying conversations, evaluated
by a new `jp_query` crate and exposed through a `--filter EXPR` flag on `jp c
ls`, `jp c grep`, `jp c rm`, `jp c archive`, and `jp c fork`.
The language supports boolean predicates over conversation metadata, the
configuration tree, and conversation events, with `event(...)` and `turn(...)`
scopes to bind predicates to a single record.

## Motivation

Locating a conversation requires combining three separate mechanisms:

- `jp c ls` filters by metadata flags (`--archived`, `--local`, `--sort`).
- `jp c grep` searches event content textually.
- `ConversationTarget` keywords (`?`, `?p`, `last`, `+session`) resolve to
  specific IDs.

None of these answer combined or structured queries: which conversation called
`fs_modify_file` with `path = crates/jp_cli/src/lib.rs`, which archived
conversations used a specific model, or which conversations contain a chat
response mentioning X and are older than a week.
Answering those requires post-processing JSON or chaining commands with brittle
text matching, and neither approach reaches structured event fields (tool name,
tool arguments) without coupling to the on-disk JSON shape.

Tool call arguments are also base64-encoded on disk to keep them out of editor
and `rg` results, so any structured search must decode them through the
conversation runtime.
Only a JP-native query engine can inspect those fields directly.

A shared predicate language handles all of these queries, works across
subcommands, and gives plugins and scripts a single query interface.

## Design

### CLI Surface

Five subcommands gain a `--filter EXPR` flag:

- `jp c ls`: list matching conversations.
- `jp c grep`: search text within matching conversations.
- `jp c rm`: remove matching conversations (subject to the confirmation rules
  below).
- `jp c archive`: archive matching conversations.
- `jp c fork`: fork matching conversations.

<!-- end list -->

```sh
jp c ls --filter 'archived and assistant.model == "anthropic/claude-sonnet-4-5"'
jp c ls --filter 'tool == "fs_modify_file" and arg.path == "crates/jp_cli/src/lib.rs"'
jp c grep --filter 'tool == "fs_modify_file"' 'TODO'
jp c rm  --filter 'created < "1 month ago" and not pinned'
```

The flag accepts an expression directly, `@path/to/file.qry` to read from a
file, or `-` to read from stdin.

### Grammar

```
expr      := or_expr
or_expr   := and_expr ('or' and_expr)*
and_expr  := not_expr ('and' not_expr)*
not_expr  := 'not' not_expr | primary
primary   := '(' expr ')' | scope | predicate
scope     := ('event' | 'turn') '(' expr ')'
predicate := field (op value)?

field        := dotted_field | bare_field
dotted_field := '.' segment ('.' segment)*
bare_field   := ident ('.' segment)*
segment      := ident | quoted_string
ident        := [a-zA-Z_][a-zA-Z0-9_-]*

op       := '==' | '!=' | 'contains' | '~' | '<' | '>' | '<=' | '>='
value    := string | number | bool
string   := '"' (escape | non_dquote_non_backslash)* '"'
          | "'" non_squote* "'"
number   := signed integer or floating point literal
bool     := 'true' | 'false'
escape   := '\n' | '\t' | '\r' | '\"' | '\\' | '\u{HEX}'
```

Operator precedence is `not` > `and` > `or`, and parentheses override
precedence.

Boolean fields can omit the operator and value: `archived` is shorthand for
`archived == true`, and `not pinned` negates `pinned == true`.

### Quoting

Strings use either double or single quotes:

- `"..."` processes escape sequences: `\n`, `\t`, `\r`, `\"`, `\\`, and
  `\u{HEX}` (Rust-style, 1 to 6 hex digits).
- `'...'` is a raw string with no escape processing.

Both forms produce the same string type, which simplifies shell quoting:

```sh
jp c ls --filter "title contains '$FOO'"   # shell expands $FOO
jp c ls --filter 'title contains "$FOO"'   # literal $FOO
```

### Field Paths

Field paths have two equivalent forms, bare and dot-prefixed:

```
tool                         # canonical, bare
arg.path                     # canonical, bare with chain
arg."request body"           # bare first, quoted later
metadata."x-trace-id".value  # mixed
."weird field"               # quoted first segment requires the dot
."weird field".sub           # same, with continuation
"weird field"                # invalid — parses as string literal
```

The leading dot is required only when the first segment is quoted or collides
with a reserved keyword (`and`, `or`, `not`, `true`, `false`, `event`, `turn`),
such as `.and`.
After the first segment, the `.` separator already distinguishes a quoted path
segment from a string literal.
Documentation and error messages format field paths in the bare form unless the
first segment requires a leading dot.

### Field Namespace

A static registry defines three top-level field roots:

| Root                  | Cost                  | Examples                                                                       |
| --------------------- | --------------------- | ------------------------------------------------------------------------------ |
| Conversation metadata | cheap (no event load) | `title`, `archived`, `pinned`, `local`, `created`, `updated`, `messages`       |
| Configuration tree    | cheap (no event load) | `assistant.model`, `assistant.reasoning.enabled`, `conversation.tool.style`, … |
| Event fields          | event load required   | `event`, `tool`, `arg.*`, `content`                                            |

Configuration-tree fields mirror the `AppConfig` schema.
Only primitive config fields (`string`, `number`, `bool`) are queryable in v1;
referencing a list or map field is a parse-time error.

Unknown fields also fail at parse time rather than silently matching nothing.
A typo such as `archvied` returns `unknown field 'archvied'` with source
position.

### Types

The language has four primitive types: `string`, `number`, `bool`, and `date`.
Numbers are `i64` or `f64`, and mixed comparisons promote integers to floats.

When a date field is compared against a string literal, the right-hand side is
parsed in one of two forms:

- **Absolute** (RFC 3339 / ISO 8601): `"2026-01-01"`, `"2026-01-01T12:00:00Z"`.
- **Relative**: `"now"`, or `"N <unit> ago"` where `<unit>` is `second`,
  `minute`, `hour`, `day`, `week`, `month`, or `year` (singular or plural).

<!-- end list -->

```
created > "2026-01-01"
created > "1 day ago"
updated > "2 weeks ago"
created < "now"
```

The parser tries RFC 3339 first, then relative time, and returns a parse error
if neither matches.

Type mismatches (`messages > "ten"`, `archived < 3`) produce a parse error when
the field type is known statically, and a runtime evaluation error otherwise.
Strings are never implicitly coerced to numbers.

### Semantics

Each field has an **intrinsic record level**: conversation, turn, or event.
Expressions evaluate against a conversation under five rules:

1. **Same-level predicates bind to the same record.** Joining two predicates at
   the same record level binds them to one record: `tool == "X" and arg.path ==
   "Y"` requires both predicates to match the same event.

2. **Higher-level fields broadcast across lower-level predicates.**
   Conversation-level fields act as constants when joined with turn- or
   event-level predicates.
   `assistant.model == "X" and tool == "Y"` matches when the conversation's
   model is `X` and at least one event has `tool == "Y"`.

3. **Unscoped event-level predicates are existential.** At the top level, `tool
   == "X"` matches any conversation that contains at least one event with `tool
   == "X"`.

4. **Negating an event-level predicate is universal.** `not tool == "X"` means
   no event in the conversation has `tool == "X"`, equivalent to `not event(tool
   == "X")`.

5. **Explicit scoping with `event(...)` and `turn(...)`.** Wrap predicates in
   `event(...)` or `turn(...)` to bind them to an explicit scope:

<!-- end list -->

```
# Default: same-event binding (tool name + tool arg on the same call)
tool == "fs_modify_file" and arg.path == "crates/jp_cli/src/lib.rs"

# Cross-event: called X in some event AND Y in some (possibly different) event
event(tool == "X") and event(tool == "Y")

# Same-turn binding: both calls happened within one turn
turn(tool == "X" and tool == "Y")
```

`TurnStart` event boundaries in the conversation stream delimit turns, matching
the [ubiquitous language][ubiquitous] definition.
Inside `event(...)` or `turn(...)`, conversation-level fields still broadcast as
constants.
Nested scopes (`turn(event(P))`) are valid, and redundant nesting is allowed.

### Cost-Aware Execution

The evaluator determines from the AST alone whether an expression references any
event-level fields.
Expressions that reference only conversation metadata and configuration **must
not load event streams**, so `jp c ls --filter 'archived'` performs the same I/O
as `jp c ls --archived`.

### Destructive Command UX

On destructive commands (`jp c rm`, `jp c archive`), a mistyped `--filter`
expression or unexpected operator precedence can match many conversations at
once.
To guard against accidental bulk changes:

- A destructive command with `--filter` prints the matching conversation IDs and
  prompts for confirmation unless `--yes` is passed.
- `-F json` output mode bypasses the interactive prompt; scripts pass `--yes`
  explicitly to suppress prompts.
- `--dry-run` prints the matching conversations without modifying them.

### Architecture

A new `jp_query` crate owns the parser, AST, field registry, and evaluator:

- `Expression`: the typed AST.
- `parse(s: &str) -> Result<Expression, ParseError>`: parses an expression and
  returns a position-aware error on failure.
- `Expression::touches_events(&self) -> bool`: reports whether the expression
  references event-level fields.
- `Expression::evaluate(&self, ctx: &EvaluationContext) -> bool`: evaluates the
  expression against `EvaluationContext`, which holds conversation metadata,
  merged config, and an optional event stream.

`jp_query` depends on `jp_conversation` (for `ConversationStream` and
`EventKind`) and `jp_config` (for the config schema).
It does not depend on `jp_workspace`, `jp_cli`, or any storage backend, and the
evaluator performs no I/O.

`jp_cli` defines a `FilterArg` `clap` type and flattens it into each subcommand
that supports `--filter`.
`FilterArg` accepts `EXPR`, `@FILE`, and `-` (stdin).

## Drawbacks

**A second small grammar in the codebase.** `ConversationTarget` already defines
a compact selector syntax (`?`, `+session`).
Adding a filter DSL introduces a second domain-specific syntax.
Although the two live at different layers (selector versus predicate) and do not
compose textually, contributors must maintain both.

**Hyrum's Law on field names.** Every config key and conversation metadata field
becomes queryable, so renaming any field is a breaking change for filter
expressions.
Renames require coordinated updates to the field registry, documentation, and
external scripts.

**Cost-model surprise from the same flag.** `--filter 'archived'` reads only
metadata, whereas `--filter 'tool == "X"'` loads event streams.
Cost-aware execution avoids unnecessary I/O automatically, but the CLI surface
gives no indication of why one query is fast and another is slow.

## Alternatives

### Flag-Based Predicates

Add flags such as `--tool NAME` and `--arg KEY=VALUE` to a new `jp c find`
command instead of introducing a DSL.

Rejected because CLI flags compose poorly for boolean logic.
Expressing `and`, `or`, and `not` across predicates requires either a
combinatorial set of flags (`--tool`, `--not-tool`, `--any-tool`, `--all-tool`)
or fixed implicit-`and` semantics that cannot express `or` queries or bind
multiple flags to the same event.

### Reuse `jq` via `jaq-core`

Accept `jq` syntax for filter expressions, wrap user input in `select(...)`
internally, and feed `jq` a synthetic per-event JSON view.

Rejected because `jq` does not share the query language's semantic model.
Record-level binding, conversation-field broadcasting, and `turn(...)` /
`event(...)` scoping would still require a custom semantic layer on top of `jq`.
Inspecting `jq`'s permissive AST for cost-aware execution is also harder than
inspecting a dedicated predicate AST, and neither relative date literals (`"1
day ago"`) nor shell-friendly single-quoted strings fit `jq` syntax naturally.
Adding `jaq-core` (~5 kLoC) would bring significant dependency weight for little
gain once custom semantics are layered on top.

### Metadata-Only Filters

Restrict `--filter` to conversation metadata and configuration fields, omitting
event predicates.

Rejected because the primary motivating query ("which conversation modified file
X?") requires inspecting tool call events.

### Defer Relative-Time Syntax to v2

Support only RFC 3339 timestamps in v1 and defer `"N <unit> ago"` literals to
v2.

Rejected because relative timestamps are much easier to write in interactive
time-based queries, require roughly 100 lines of isolated parser code, and fit
into the string-literal date rule without changing the grammar.

## Non-Goals

- **Transformations.** The DSL is a boolean predicate language, not a pipeline
  language.
  Use `jq` for output reshaping (`jp c show -F json | jq ...`).
- **Aggregations.** v1 does not support `count(...)`, `sum(...)`, `min(...)`, or
  `max(...)`.
- **List and map config values.** Collection-typed config keys such as
  `conversation.tool.allow` are not queryable in v1; supporting them requires
  defining `in`, `subset`, and `any(...)` operators.
- **Cross-event temporal ordering.** Queries cannot express ordering across
  events ("called X and then later called Y").
  Use `turn(...)` for same-turn binding; broader temporal queries are deferred.
- **Custom functions.** v1 includes neither user-defined functions nor built-in
  scalar functions (`upper`, `length`).
- **Date arithmetic in function form.** Expressions such as `created > now() -
  "1 day"` are deferred; relative string literals (`"1 day ago"`) cover v1.
- **`ConversationTarget` keyword integration.** Target keywords (`?`, `last`,
  `+session`) remain selectors separate from `--filter`.
  They compose externally by resolving targets first and filtering the resulting
  set.
- **Saved queries and aliases.** Named query aliases are out of scope for v1.

## Risks and Open Questions

### Final Field Inventory

The field categories are settled, but the exact v1 list of conversation-metadata
and event fields must be finalized during implementation.

### Shape of the `event` Field

The `event` field has two candidate shapes:

- Flat: `event == "tool_call_request"` (string matching the event kind tag).
- Nested: `event.kind == "tool_call_request"` (struct).

The flat form is simpler, matches the rest of the predicate language, and is the
default unless implementation surfaces a reason to nest subfields under `event`.

### Error Reporting Quality

Parse errors must include source positions, and type-mismatch errors must point
at the offending field and report its registered type rather than returning a
generic `ParseError("syntax error")`.

### Destructive Command Behavior Contract

The `--dry-run`, confirmation prompt, and `--yes` rules in the Design section
become part of the public CLI contract once shipped and should be validated
during review against both interactive and scripted workflows.

### Long Expressions

`--filter @file.qry` and `--filter -` (stdin) are part of v1, but file-loading
details (encoding, comment syntax, and multi-line formatting) still need to be
specified.

## Implementation Plan

### Phase 1: `jp_query` Crate

Create `jp_query` with the AST types, parser, static field registry
(conversation metadata, primitive config fields, and event fields),
`Expression::touches_events()`, and evaluator over `&EvaluationContext`.
Add tests for parser round-trips, evaluation semantics on representative event
streams, and error messages.
This phase is reviewable and mergeable on its own.

### Phase 2: `--filter` on Read-Only Commands

Add `FilterArg` to `jp_cli` and integrate `--filter` into `jp c ls` and `jp c
grep`, loading event streams only when `Expression::touches_events()` returns
`true`.
Depends on Phase 1.

### Phase 3: `--filter` on Destructive Commands

Add `--filter` to `jp c rm`, `jp c archive`, and `jp c fork`, implement the
`--dry-run`, confirmation prompt, and `--yes` behavior, and add end-to-end tests
for the safety paths.
Depends on Phase 2 to share `FilterArg` and dispatch wiring.

### Phase 4: Documentation

Add user-facing documentation and examples under `docs/features/`, and generate
the config-field registry from the `AppConfig` schema where possible to prevent
drift.

## References

- [RFD 050]: Scripting Ergonomics for Conversation Management, which defines the
  broader scripting workflow this DSL supports.
- [`docs/wiki/00dxybd-ubiquitous-language.md`][ubiquitous]: definitions of
  Conversation, Turn, Event, and related terms used throughout this RFD.

[RFD 050]: ../050-scripting-ergonomics-for-conversation-management.md
[ubiquitous]: ../../wiki/00dxybd-ubiquitous-language.md
