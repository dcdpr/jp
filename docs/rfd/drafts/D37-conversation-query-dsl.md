# RFD D37: Conversation Query DSL

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-16

## Summary

Introduce a small predicate language for querying conversations, evaluated by
a new `jp_query` crate and exposed through a `--filter EXPR` flag on `jp c ls`,
`jp c grep`, `jp c rm`, `jp c archive`, and `jp c fork`. The language supports
boolean predicates over conversation metadata, the configuration tree, and
conversation events, with explicit `event(...)` and `turn(...)` scoping for
binding predicates to a single record.

## Motivation

Locating a conversation today means combining several mechanisms:

- `jp c ls` filters by metadata flags (`--archived`, `--local`, `--sort`).
- `jp c grep` searches event content textually.
- `ConversationTarget` keywords (`?`, `?p`, `last`, `+session`) resolve to
  specific IDs.

None of them answer questions like "which conversation called `fs_modify_file`
with `path = crates/jp_cli/src/lib.rs`?", "which archived conversations used a
specific model?", or "which conversations contain a chat response mentioning X
*and* are older than a week?". Composing those queries today requires
post-processing JSON or chaining commands with brittle text matching, and even
then no path gets at structured event fields (tool name, tool arguments) without
exposing the on-disk JSON shape.

Tool call arguments are also base64-encoded on disk specifically to keep them
out of editor and `rg` results, so any structured search must round-trip
through the conversation runtime. That makes a JP-native query engine the
natural — and effectively the only — place for this capability.

A single predicate language solves these cases with one mechanism, composes
naturally across multiple commands, and gives plugins and scripting workflows
a programmatic surface for finding conversations.

## Design

### Surface

A new `--filter EXPR` flag is added to:

- `jp c ls` — show matching conversations.
- `jp c grep` — search text within matching conversations.
- `jp c rm` — remove matching conversations (with destructive-UX rules below).
- `jp c archive` — archive matching conversations.
- `jp c fork` — fork all matching conversations.

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

Operator precedence: `not` > `and` > `or`. Parentheses override.

A predicate may omit the operator and value when the field is boolean-typed:
`archived` is sugar for `archived == true`. `not pinned` reads as expected.

### Quoting

Both single and double quotes delimit strings:

- `"..."` — escapes processed: `\n`, `\t`, `\r`, `\"`, `\\`, `\u{HEX}`
  (Rust-style; 1–6 hex digits).
- `'...'` — raw, no escapes.

Both produce semantically identical strings. The split exists for shell
composition:

```sh
jp c ls --filter "title contains '$FOO'"   # shell expands $FOO
jp c ls --filter 'title contains "$FOO"'   # literal $FOO
```

### Field paths

Two equivalent forms:

```
tool                         # canonical, bare
arg.path                     # canonical, bare with chain
arg."request body"           # bare first, quoted later
metadata."x-trace-id".value  # mixed
."weird field"               # quoted first segment requires the dot
."weird field".sub           # same, with continuation
"weird field"                # invalid — parses as string literal
```

The dot prefix is only required when the *first* segment must be quoted; once
past the first segment, a leading `.` already disambiguates a path
continuation from a string literal.

Documentation and error messages use the bare form unless the first segment
must be quoted. A field name that collides with a reserved keyword (`and`,
`or`, `not`, `true`, `false`, `event`, `turn`) is escaped with a leading dot:
`.and`.

### Field namespace

Three top-level roots, declared in a static registry:

| Root                  | Cost                  | Examples                                                                              |
|-----------------------|-----------------------|---------------------------------------------------------------------------------------|
| Conversation metadata | cheap (no event load) | `title`, `archived`, `pinned`, `local`, `created`, `updated`, `messages`              |
| Configuration tree    | cheap (no event load) | `assistant.model`, `assistant.reasoning.enabled`, `conversation.tool.style`, …        |
| Event fields          | event load required   | `event`, `tool`, `arg.*`, `content`                                                   |

Configuration-tree fields mirror the `AppConfig` schema exactly. Only
primitive-typed config fields (string, number, bool) are queryable in v1; list
and map types fall back to a parse-time error.

Unknown fields are a **parse-time error**, not a silent "no match." A typo of
`archvied` returns a clear `unknown field 'archvied'` with position
information.

### Types

Four primitive types: `string`, `number`, `bool`, `date`.

Numbers are `i64` and `f64`; mixed comparison promotes int to float.

Dates parse from string literals on the right side of a comparison when the
field is date-typed. Two forms accepted:

- **Absolute** (RFC 3339 / ISO 8601): `"2026-01-01"`,
  `"2026-01-01T12:00:00Z"`.
- **Relative**: `"N <unit> ago"` where `<unit>` is one of `second`, `minute`,
  `hour`, `day`, `week`, `month`, `year` (plural accepted). Also `"now"`.

```
created > "2026-01-01"
created > "1 day ago"
updated > "2 weeks ago"
created < "now"
```

Resolution order on the right side of a date comparison: RFC 3339 →
relative-time → parse-time error.

Type mismatches (`messages > "ten"`, `archived < 3`) are parse-time errors
where statically detectable, runtime errors otherwise. No implicit
string-to-number coercion.

### Semantics

Each field has an **intrinsic record level**: conversation, turn, or event.
Expressions evaluate against a conversation, with these rules:

1. **Same-record-level binding is the default.** Predicates over fields at the
   same record level are joined at that level. `tool == "X" and arg.path ==
   "Y"` — both event-level — bind to the *same event*.

2. **Cross-level mixing broadcasts the higher-level field.**
   `assistant.model == "X" and tool == "Y"` reads as "the conversation's model
   is X *and* there exists an event with tool=Y." Conversation-level fields
   act as constants when joined with event-level predicates.

3. **Event-level predicates are existentially quantified at the top level.**
   `tool == "X"` matches conversations that contain at least one event with
   tool=X.

4. **`not` over event-level predicates is universal.** `not tool == "X"`
   means "no event in this conversation has tool=X" — the natural reading,
   and the one that makes negation reachable without de Morgan acrobatics.

5. **Explicit scoping with `event(...)` and `turn(...)`.** Force a binding
   scope when the default isn't enough:

```
# Default: same-event binding (tool name + tool arg on the same call)
tool == "fs_modify_file" and arg.path == "crates/jp_cli/src/lib.rs"

# Cross-event: called X in some event AND Y in some (possibly different) event
event(tool == "X") and event(tool == "Y")

# Same-turn binding: both calls happened within one turn
turn(tool == "X" and tool == "Y")
```

A *turn* is defined by `TurnStart` event boundaries in the conversation
stream — the existing ubiquitous-language definition.

Inside `event(...)` or `turn(...)`, conversation-level fields are broadcast as
constants (same rule as at the top level). Nested scopes (`turn(event(P))`)
are accepted; redundant nesting is not rejected.

### Cost-aware execution

The evaluator computes, from the AST alone, whether an expression references
any event-scoped fields. Expressions that touch only conversation metadata
and configuration **must not load event streams**.

This is non-negotiable: it is cheap to enforce at design time and expensive
to retrofit. `jp c ls --filter 'archived'` runs at the same cost as `jp c ls
--archived`.

### Destructive command UX

`jp c rm --filter`, `jp c archive --filter`, and similar destructive commands
carry a real risk: a typo or an unexpected operator semantic can affect many
conversations at once.

The rule:

- A destructive command with `--filter` prints a list of affected conversation
  IDs and asks for confirmation, unless `--yes` is passed.
- `-F json` output mode bypasses the prompt; scripts pass `--yes` explicitly
  to suppress prompts.
- A `--dry-run` flag shows what would be affected without doing it.

This makes the safety behavior part of the command's contract from day one,
before scripts accumulate.

### Architecture

A new crate `jp_query` owns the parser, AST, type registry, and evaluator:

- `Expression` — typed AST.
- `parse(s: &str) -> Result<Expression, ParseError>` — produces a
  position-aware parse error on failure.
- `Expression::touches_events(&self) -> bool` — for cost-aware dispatch.
- `Expression::evaluate(&self, ctx: &EvaluationContext) -> bool` —
  `EvaluationContext` carries the conversation metadata, merged config, and an
  optional event stream.

The crate depends on `jp_conversation` (for `ConversationStream`, `EventKind`)
and `jp_config` (for the config schema). It does not depend on `jp_workspace`,
`jp_cli`, or any storage backend. The evaluator is pure — no I/O.

`jp_cli` adds a single `FilterArg` clap type, flattened into each subcommand
that supports `--filter`. The arg type accepts `EXPR`, `@FILE`, and `-`
(stdin).

## Drawbacks

**A second small grammar in the codebase.** `ConversationTarget` already has a
tiny grammar (`?`, `+session`, etc.). This adds another. They live at
different layers (selector vs. predicate) and do not compose textually, but
contributors now have two mini-languages to keep in mind.

**Hyrum's Law on field names.** Every config key and every conversation
metadata field becomes queryable, which means renaming any of them is a
DSL-breaking change. The field registry is the load-bearing surface; rename
moves require coordinated changes to the registry, the docs, and any external
scripts.

**Cost-model surprise from the same flag.** `--filter 'archived'` is cheap;
`--filter 'tool == "X"'` is expensive. The same flag, two cost regimes.
Cost-aware execution hides the regime from users — which is the right
tradeoff for ergonomics, but means a user can't tell from the surface why one
query is fast and another is slow.

## Alternatives

### Flag-based predicates (no DSL)

Add `--tool NAME`, `--arg KEY=VALUE`, etc. to a new `jp c find` command.

Rejected. Composes poorly: AND/OR/NOT across predicates requires either flag
explosion (`--tool`, `--not-tool`, `--any-tool`, `--all-tool`) or implicit-AND
semantics that can't express the motivating query. Same-event binding via
flags is unnatural. Path Independence: rebuilding JP today with current
scripting ambitions would not land on flags.

### Reuse jq via `jaq-core`

Accept jq syntax for filter expressions, wrap user input in `select(...)`
internally, feed jq a synthetic per-event JSON view.

Rejected. The semantic model we want (record-level binding, conversation
broadcast, `turn`/`event` scoping) is not native to jq — we would own the
entire semantic layer anyway, with jq serving as a low-level boolean
evaluator. Cost-aware execution becomes much harder: jq's AST is more
permissive than our predicate AST, validation has to walk it twice. Two v1
requirements — `"1 day ago"` syntax and shell-friendly single quotes — are
not natural in jq. Net dependency cost (~5kLoC of `jaq-core`) for a benefit
that erodes once the semantics are layered on top.

### Predicates on metadata-only filters

Skip the event-content angle entirely; `--filter` operates only on
conversation metadata and config.

Rejected. The motivating use case — "which conversation modified file X?" —
needs event predicates. A metadata-only filter doesn't solve the problem this
RFD exists for.

### Defer relative-time syntax to v2

Originally proposed during design. Reversed: relative times are a major
ergonomic win on time-based queries, cost ~100 lines of isolated parser code,
and fit cleanly into the existing string-literal date rule with no grammar
impact.

## Non-Goals

- **Transformations.** Output reshaping (`jp c show -F json | jq ...`) is
  jq's job. The DSL is a predicate language, not a pipeline language.
- **Aggregations.** No `count(...)`, `sum(...)`, `min(...)`, `max(...)` in v1.
- **List / map config values.** `conversation.tool.allow` and similar
  collection-typed config keys are not queryable in v1. Adding support
  requires defining `in`, `subset`, `any(...)` operators.
- **Cross-event temporal ordering.** "Called X and *then later* called Y" is
  not expressible. Use `turn(...)` for same-turn binding; broader temporal
  queries are deferred.
- **Custom functions.** No user-defined functions, no built-in scalar
  functions (`upper`, `length`, etc.).
- **Date arithmetic in function form.** `created > now() - "1 day"`-style
  expressions are deferred. The string-literal form (`"1 day ago"`) covers
  v1.
- **`ConversationTarget` keyword integration.** Keywords (`?`, `last`,
  `+session`) remain selectors, separate from `--filter`. They compose
  externally: resolve targets, then filter the set.
- **Saved queries and aliases.** Out of scope for v1.

## Risks and Open Questions

### Final field inventory

The exact v1 list of conversation-metadata and event fields needs ratifying
in the implementation. The categories are settled; the exact names and shapes
are not.

### Shape of the `event` field

Two candidate forms:

- Flat: `event == "tool_call_request"` (string of the kind tag).
- Nested: `event.kind == "tool_call_request"` (struct).

The flat form is simpler and matches the predicate-language style. Pinned as
default unless implementation surfaces a reason to nest.

### Error reporting quality

Position-aware parse errors are a hard requirement; type-mismatch errors
should point at the offending field and surface its registered type.
`ParseError("syntax error")` is not acceptable. The implementation commits
to a clear error story up front.

### Destructive command behavior contract

The dry-run / confirmation / `--yes` rules in the Design section are part of
the public contract once shipped. Worth one more pass during review to make
sure the defaults match user expectations.

### Long expressions

`--filter @file.qry` and `--filter -` (stdin) are part of v1, but the
file-loading details (encoding, comment syntax, multi-line formatting) are
not yet specified.

## Implementation Plan

### Phase 1: `jp_query` crate

Create `jp_query` with AST types, parser, and evaluator. Define the static
field registry covering conversation metadata, the config tree (primitives),
and event fields. Implement `Expression::touches_events()` for cost-aware
dispatch. Implement the evaluator over `&EvaluationContext`. Tests: parser
round-trips, semantic correctness on representative streams, error-message
coverage.

Reviewable and mergeable independently.

### Phase 2: `--filter` on read-only commands

Add `FilterArg` to `jp_cli` and integrate `--filter` on `jp c ls` and
`jp c grep`. Cost-aware dispatch is exercised here; event-loading kicks in
only when the expression references event fields.

Depends on Phase 1.

### Phase 3: `--filter` on destructive commands

Add `--filter` to `jp c rm`, `jp c archive`, and `jp c fork`. Implement the
dry-run / confirmation / `--yes` UX. End-to-end tests for the safety paths.

Depends on Phase 2 (to share the `FilterArg` type and dispatch wiring).

### Phase 4: Documentation

User-facing documentation under `docs/features/`, with a concentrated
examples section. Where possible, the field registry is generated from the
config schema to avoid drift.

## References

- [RFD 050]: Scripting Ergonomics for Conversation Management — the broader
  scripting story this DSL plugs into.
- [`docs/architecture/ubiquitous-language.md`][ubiquitous] — definitions of
  Conversation, Turn, Event, and related terms used throughout this RFD.

[RFD 050]: ../050-scripting-ergonomics-for-conversation-management.md
[ubiquitous]: ../../architecture/ubiquitous-language.md
