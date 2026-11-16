# RFD D25: Argument-Conditional Tool Policy

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-24
- **Summary**: Makes tool `run` and `result` policies conditional on tool call
  arguments via ordered first-match-wins rules.

## Summary

This RFD makes tool `run` and `result` policies (`ask`, `unattended`, `edit`,
and `skip`) conditional on tool call arguments instead of applying a single mode
to every invocation.
The `run` and `result` settings move under a `policy` namespace and accept
either a mode string or an ordered list of argument-matching rules evaluated
with first-match-wins semantics.

## Motivation

Tool `run` configuration currently accepts a single `RunMode` that applies to
every call regardless of its arguments.
This per-tool granularity forces a trade-off between safety and flow:

1. **Overly permissive.** Setting `fs_modify_file.run = "unattended"` lets the
   assistant modify any file without approval, including `.env`, `Cargo.toml`,
   or files outside the working area.

2. **Overly restrictive.** Setting `fs_modify_file.run = "ask"` prompts on every
   file modification, including routine edits under `src/`.
   Constant prompts interrupt flow and train users to approve without reading.

3. **No middle ground.** There is no way to run edits under `src/` unattended
   while prompting for every other path.

The same limitation applies to `result` (how tool results are delivered to the
assistant) and to any future per-invocation policy.

### Relationship to RFD 076 and D04

[RFD 076] defines typed access grants (`AccessPolicy`) that declare which
resources a tool can access, and [RFD 075] enforces those grants at the OS
level.
This RFD controls whether JP prompts the user before the tool runs.

| Concern         | RFD 076 / D04                         | This RFD                           |
| --------------- | ------------------------------------- | ---------------------------------- |
| **Question**    | "Can this tool access this resource?" | "Should JP prompt before running?" |
| **Enforcement** | Tool self-check + OS sandbox          | Coordinator prompt logic           |
| **Granularity** | Per-path capabilities                 | Per-argument run mode              |
| **Config key**  | `access`                              | `policy.run`                       |

The two systems are complementary.
A tool may have broad access rights (095) but still require approval for
sensitive arguments, or it may run unattended because its access policy already
constrains what it can touch.
Both systems use the `path` parameter type introduced in this RFD and can share
path normalization and prefix-matching logic.

## Design

### The `policy` namespace

A new `policy` section in tool configuration groups execution policy settings:

```toml
[conversation.tools.fs_modify_file.policy]
run = "ask"
result = "unattended"
```

The existing top-level `run` and `result` fields continue to work as aliases for
backward compatibility:

```toml
# These are equivalent:
[conversation.tools.fs_modify_file]
run = "ask"

[conversation.tools.fs_modify_file.policy]
run = "ask"
```

When both forms are present, `policy.run` and `policy.result` take precedence
and the top-level fields are ignored with a deprecation warning.

The `policy` namespace also applies to `ToolsDefaultsConfig`:

```toml
[conversation.tools.*.policy]
run = "ask"
result = "unattended"
```

### Run policy: string or rules array

The `policy.run` field accepts either a mode string or an ordered array of
conditional rules:

```toml
# String alias (equivalent to a single catch-all rule):
run = "ask"

# Array of rules:
run = [
    { arg = "/path", prefix = "src/sensitive/", mode = "ask" },
    { arg = "/path", prefix = "src/", mode = "unattended" },
    { arg = "/path", prefix = ".env", mode = "ask" },
    { mode = "ask" },
]
```

The string aliases `"ask"`, `"unattended"`, `"edit"`, and `"skip"` desugar to
`[{ mode = "<value>" }]`, a single catch-all rule with no condition.
A plain mode string requires no argument inspection and can be decided before
any arguments arrive.

### Rule structure

Each rule has an optional condition (`arg` plus a matcher keyword) and a `mode`
(`RunMode`).
A rule with no condition is a catch-all that matches any invocation:

```toml
# Condition on a top-level parameter:
{ arg = "/path", prefix = "src/", mode = "unattended" }

# Condition on a nested parameter (see "Argument paths" below):
{ arg = "/patterns/paths", prefix = ".env", mode = "ask" }

# Catch-all (no condition):
{ mode = "ask" }
```

Compound conditions that match multiple parameters at once are out of scope (see
[Non-Goals](#non-goals)).

### Evaluation: first-match-wins

Rules are evaluated top to bottom, and the first matching rule determines the
run mode.
If no rule matches, evaluation falls back to an implicit `{ mode = "ask" }`.

```toml
run = [
    { arg = "/path", prefix = "src/sensitive/", mode = "ask" },
    { arg = "/path", prefix = "src/", mode = "unattended" },
    { mode = "ask" },
]
```

For `path = "src/sensitive/secret.rs"`:

- Rule 1: prefix `"src/sensitive/"` matches → **`ask`**

For `path = "src/lib.rs"`:

- Rule 1: prefix `"src/sensitive/"` does not match → skip
- Rule 2: prefix `"src/"` matches → **`unattended`**

For `path = "README.md"`:

- Rule 1: no match → skip
- Rule 2: no match → skip
- Rule 3: catch-all → **`ask`**

Declaration order controls priority: specific rules come first, followed by
broader rules and a final catch-all, matching the evaluation model of firewall
rules, nginx location blocks, and route tables.

### Argument paths (JSON Pointer)

The `arg` field uses [JSON Pointer (RFC 6901)][rfc-6901] syntax to identify
which parameter value to match against:

| `arg` value       | Resolves to                          |
| ----------------- | ------------------------------------ |
| `/path`           | Top-level `path` parameter           |
| `/patterns/paths` | Nested: each pattern's `paths` array |
| `/patterns/old`   | Nested: each pattern's `old` field   |
| `/source`         | Top-level `source` parameter         |

JSON Pointer parsing uses the [`jsonptr`][jsonptr] crate, which implements RFC
6901 with `serde_json` integration.

**Array traversal.** Standard JSON Pointer uses numeric indices to address
specific array elements (`/patterns/0/paths/0`).
For run policy evaluation, the pointer navigates the tool's **parameter schema**
rather than a single document instance.
When a pointer segment resolves to an array type in the schema, it matches
against any element: the evaluator tests the matcher against every element at
that position in the call arguments.

For example, given `arg = "/patterns/paths"` and arguments:

```json
{
  "patterns": [
    {
      "old": "foo",
      "new": "bar",
      "paths": [
        "src/a.rs"
      ]
    },
    {
      "old": "x",
      "new": "y",
      "paths": [
        ".env"
      ]
    }
  ]
}
```

The evaluator checks `patterns[0].paths[0]` (`"src/a.rs"`), any remaining
elements in `patterns[0].paths`, `patterns[1].paths[0]` (`".env"`), and so on.
The condition is met if **any** resolved element satisfies the matcher.

Existential ("any element matches") semantics is the safe default: tool calls
execute atomically, so a single sensitive value anywhere in the arguments must
trigger the matching rule.

**Validation at config load time.** JP walks the tool's `ToolParameterConfig`
tree along the pointer segments.
Each segment must resolve to a valid property or array item type; traversing a
scalar field or referencing a nonexistent property is a config error.

### The `path` parameter type

This RFD introduces `path` as a parameter type alongside `string`, `number`,
`integer`, `boolean`, `array`, and `object`:

```toml
[conversation.tools.fs_modify_file.parameters.path]
type = "path"
summary = "The path to the file to modify."
```

The `path` type affects two behaviors:

1. **JSON Schema generation.** `path` is a JP-internal semantic annotation.
   `to_json_schema()` emits `"type": "string"`, so LLMs see a standard string
   parameter.

2. **Matcher behavior.** On a `path` parameter, the `prefix` matcher compares
   normalized path components rather than raw byte prefixes.
   Paths are normalized before matching (resolving `.` and `..` and stripping
   trailing separators), so `prefix = "src/"` matches `"src/lib.rs"` but not
   `"src-old/lib.rs"`.

Tool definitions should use `type = "path"` for parameters that represent
filesystem paths.

### Type-constrained matchers

Each rule condition consists of an `arg` pointer and a single matcher keyword.
The valid matchers depend on the resolved parameter type, following the [JSON
Schema validation vocabulary][json-schema-validation] with a JP-specific
`prefix` extension for path matching.

#### Matchers for any type

| Keyword | JSON Schema | Description                                                |
| ------- | :---------: | ---------------------------------------------------------- |
| `const` |      ✓      | Exact value match. Accepts any JSON value.                 |
| `enum`  |      ✓      | Set membership. Value must equal one of the listed values. |

```toml
{ arg = "/util", const = "jq", mode = "ask" }
{ arg = "/util", enum = ["date", "wc", "head", "tail"], mode = "unattended" }
{ arg = "/replace_using_regex", const = true, mode = "ask" }
```

#### Matchers for `string` and `path` types

| Keyword   | JSON Schema | Description                                                                                                 |
| --------- | :---------: | ----------------------------------------------------------------------------------------------------------- |
| `pattern` |      ✓      | Regular expression match (ECMA-262 dialect).                                                                |
| `prefix`  |   ✗ (JP)    | Path-prefix match. For `path`-typed params: component-aware. For `string`-typed params: byte-length prefix. |

```toml
{ arg = "/path", prefix = "src/", mode = "unattended" }
{ arg = "/patterns/old", pattern = "rm\\s+-rf", mode = "ask" }
```

`prefix` is a JP extension to JSON Schema: path-prefix matching is the primary
use case for run policies, and regex (`pattern = "^src/"`) neither normalizes
paths nor matches on path-component boundaries.

#### Matchers for `number` and `integer` types

| Keyword             | JSON Schema | Description            |
| ------------------- | :---------: | ---------------------- |
| `minimum`           |      ✓      | Inclusive lower bound. |
| `maximum`           |      ✓      | Inclusive upper bound. |
| `exclusive_minimum` |      ✓      | Exclusive lower bound. |
| `exclusive_maximum` |      ✓      | Exclusive upper bound. |

```toml
{ arg = "/start_line", minimum = 1000, mode = "ask" }
```

Because each rule accepts one matcher keyword, range constraints (combining a
lower and upper bound on the same parameter) require compound conditions and are
out of scope.

#### Matchers for `boolean` type

Only `const` is available for boolean parameters.
`enum` is redundant because booleans have only two values.

#### Validation

At config load time, JP validates that:

1. The `arg` pointer resolves to a valid parameter in the tool's schema.
2. The matcher keyword is valid for the resolved parameter's type.
3. The matcher value's type matches the parameter (e.g., `const = true` on a
   `number` parameter is an error).

Invalid rules are rejected at config load time with an error identifying the
rule, parameter, and type mismatch.

### Unreachable rule detection

JP rejects rules that can never fire because an earlier rule on the same `arg`
always matches first.
Shadowed rules are **config errors**, not warnings, because they indicate that
the rule order contradicts the user's intent.

| Earlier rule                  | Later rule                  | Why it's unreachable                                                            |
| ----------------------------- | --------------------------- | ------------------------------------------------------------------------------- |
| `prefix = "src/"`             | `prefix = "src/sensitive/"` | Every value matching the later prefix also matches the earlier, shorter prefix. |
| `prefix = "src/"`             | `const = "src/lib.rs"`      | The const value starts with the earlier prefix.                                 |
| `enum = ["jq", "wc"]`         | `const = "jq"`              | The const value is in the earlier enum set.                                     |
| `enum = ["jq", "wc", "date"]` | `enum = ["jq", "wc"]`       | The later set is a subset of the earlier set.                                   |
| Catch-all `{ mode = "ask" }`  | Any rule after it           | A catch-all matches everything; nothing after it can fire.                      |

Each case above is provably unreachable under first-match-wins evaluation, so
detection has no false positives.

When a rule array has no trailing catch-all rule, JP emits a warning and appends
an implicit `{ mode = "ask" }`.

### Interaction with argument streaming

String aliases and catch-all-only policies have no conditions, so JP resolves
them before any arguments stream in.

Conditional rules are evaluated once the complete argument object arrives.
Because each rule's `arg` pointer names the exact parameter it inspects, a
future optimization can evaluate rules incrementally as individual parameters
finish streaming.

> [!TIP]
> [RFD D26] defines this streaming evaluation model.

### Result policy (`policy.result`)

`policy.result` uses the same rule structure, argument paths, matchers, and
validation, with `mode` taking a `ResultMode` instead of a `RunMode`:

```toml
[conversation.tools.fs_modify_file.policy]
result = [
    { arg = "/path", prefix = ".env", mode = "ask" },
    { mode = "unattended" },
]
```

## Drawbacks

- **Ordering burden.** First-match-wins requires ordering rules from most
  specific to least specific, which can surprise users accustomed to
  specificity-based systems (such as CSS or 095's longest-prefix-match).
  Unreachable-rule errors catch shadowed rules on the same parameter, but users
  still need to understand declaration-order priority.

- **No compound conditions.** Cross-parameter conditions ("ask when `path` is in
  `src/` and regex is enabled") cannot be expressed; users must approximate them
  with broader single-parameter rules (see [Non-Goals](#non-goals)).

- **New dependency.** Adding the `jsonptr` crate for JSON Pointer parsing
  introduces a new dependency, though it has minimal transitive dependencies.

- **Migration cost.** Moving `run` and `result` under `policy` requires
  supporting the top-level fields as deprecated aliases during migration.

- **Nested argument paths.** Resolving pointers like `/patterns/paths` requires
  walking the parameter schema tree and iterating over nested array elements
  rather than looking up a top-level key.

## Alternatives

### Specificity-based matching

Evaluate all rules and pick the most specific match (longest prefix wins, exact
match beats prefix), following the [RFD 076] model for access grants.

Rejected because specificity across different matcher types has no natural
order.
Comparing two prefixes is intuitive, but ranking `const` against `prefix` or a
long prefix against `enum` requires arbitrary tie-breaking rules that users must
memorize.
First-match-wins makes priority explicit in the configuration, and
unreachable-rule detection catches the most common ordering mistake (placing a
shorter prefix before a longer one on the same parameter).

### Embed run mode in 095's `FsRule`

Attach a `run` field to `access.fs` rules so access grants and run modes share a
single configuration block.

Rejected because `access` (capability grants) and `policy.run` (confirmation
prompts) are independent concerns.
Coupling them would force users to write `access.fs` rules just to customize
prompt behavior when the default access policy is already sufficient.

### Generic predicate system with AND/OR combinators

Provide a predicate language with nested `and`, `or`, and `not` combinators
across multiple parameters.

Rejected because single-parameter conditions cover the common cases (matching
paths or command names).
Multi-parameter conjunction can be added later via an `all` key without breaking
existing configurations.

### Object-keyed `run` (grouped by parameter)

Structure `run` as an object keyed by parameter names rather than an array of
rules:

```toml
[conversation.tools.fs_modify_file.policy.run]
mode = "ask"
[conversation.tools.fs_modify_file.policy.run.path]
"src/" = "unattended"
```

Rejected because the matcher type is implicit in the key (is `"src/"` a prefix
or a `const`?), cross-parameter priority has no natural order, and future
compound conditions do not fit the table structure.

## Non-Goals

- **Compound conditions.** Matching multiple parameters simultaneously (e.g.,
  "path in `src/` AND regex enabled") is not supported.
  A future RFD can add an `all` key on rules without breaking existing
  configurations.

- **Streaming argument evaluation.** Evaluating rules as individual parameters
  stream in is left to a follow-up RFD; this RFD evaluates conditional rules
  after all arguments arrive.

> [!TIP]
> [RFD D26] extends D25 with streaming policy evaluation, consuming [RFD 043]'s
> per-parameter completion signals to resolve conditional rules before the full
> argument object arrives.

- **Access policy enforcement.** This RFD controls confirmation prompts, not
  tool capabilities.
  Access restrictions are handled by [RFD 076] (cooperative) and [RFD 075]
  (OS-level).

- **Result content inspection.** `policy.result` conditions match on the tool
  call's input arguments, not on its output (such as prompting only when output
  contains an error message).

- **MCP-specific argument handling.** MCP tool calls provide JSON arguments
  before execution and use the same policy evaluation path as built-in tools.

## Risks and Open Questions

1. **JSON Pointer array traversal semantics.** Standard JSON Pointer requires
   numeric array indices, whereas this RFD infers "any element" traversal from
   array types in the parameter schema.
   Should the config syntax require an explicit wildcard segment (such as `/-`
   or `/*`) at array positions instead?

2. **Parameter ordering for future streaming.** Incremental evaluation during
   argument streaming depends on the order in which parameters arrive.
   LLMs usually follow the JSON Schema property order (`IndexMap` iteration
   order in the tool TOML), but providers do not guarantee this.
   Should the streaming RFD validate provider ordering or degrade gracefully by
   waiting for all arguments when parameters arrive out of order?

> [!TIP]
> [RFD D26] addresses this: out-of-order arrival is graceful degradation.
> The evaluator returns `Waiting` until the needed parameter arrives.
> No incorrect decisions, just lost optimization.

3. **`prefix` on `string`-typed parameters.** On `string` parameters (as opposed
   to `path`), `prefix` performs a byte-prefix match.
   Should `prefix` be restricted to `path` parameters so `string` parameters
   must use `pattern = "^..."`?

4. **Interaction with `RunMode::Edit`.** When a conditional rule resolves to
   `mode = "edit"`, the user can modify the matched parameter (for example,
   changing `path` from `src/` to `.env`).
   In this design, JP determines the mode before the edit and does not
   re-evaluate the rules afterward.
   Documenting this behavior may be sufficient.

5. **Global defaults with conditions.** If `conversation.tools.*.policy.run`
   accepts an array of rules, its `arg` pointers apply across tools with
   different parameter schemas.
   A rule whose `arg` does not exist in a tool's schema would need to be skipped
   rather than treated as a config error.

## Implementation Plan

### Phase 1: `path` parameter type

- Recognize `"path"` in `ToolParameterConfig::kind`.
- Map `"path"` → `"string"` in `to_json_schema()`.
- Update JP's filesystem tool definitions to use `type = "path"` for path
  parameters.
- Unit tests for schema generation and type recognition.

No external dependencies.
Can merge independently.

### Phase 2: `policy` namespace

- Add `PolicyConfig` struct with `run` and `result` fields to
  `jp_config::conversation::tool`.
- Add `policy` field to `ToolConfig` and `ToolsDefaultsConfig`.
- Implement backward compatibility: top-level `run`/`result` fields are read as
  `policy.run`/`policy.result` if `policy` is absent.
- Deprecation warning when both forms are present.
- `ToolConfigWithDefaults::run()` reads from `policy.run` first, falling back to
  the top-level field, then global defaults.
- Implement `AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`, and
  `ToPartial` for the new types.

Depends on Phase 1 (for `path` type awareness in validation).

### Phase 3: Run policy types and evaluation

- Define `RunPolicy` enum (string alias or rules vec) and `RunRule` struct.
- Define `ParamCondition` and `TypedMatcher` types.
- Add `jsonptr` dependency to `jp_config` for JSON Pointer parsing.
- Implement rule evaluation: iterate rules, first match wins.
- Implement schema-walking validation: verify `arg` pointers resolve to valid
  parameters and matcher keywords are valid for the resolved type.
- Implement array traversal: when a pointer segment hits an array type, iterate
  all elements.
- Unit tests for evaluation logic, type validation, and array traversal.

Depends on Phase 2.

### Phase 4: Unreachable rule detection

- Implement shadowing detection for same-`arg` rules: prefix-shadows-prefix,
  prefix-shadows-const, enum-shadows-const, enum-shadows-enum,
  catch-all-shadows-any.
- Surface as config errors with specific messages naming the shadowing and
  shadowed rules.
- Warn when no catch-all is present.
- Unit tests for each detection case.

Depends on Phase 3.

### Phase 5: Integration with tool coordinator

- Modify `ToolExecutor::permission_info()` to return the full `RunPolicy`
  instead of a single `RunMode`.
- Modify `ToolCoordinator::decide_permission()` to evaluate the run policy
  against tool call arguments.
- For string aliases and catch-all-only policies, preserve the current fast path
  (no argument inspection needed).
- For conditional policies, evaluate after arguments are available.
- Apply the same changes for `policy.result` in result delivery.
- Integration tests verifying end-to-end behavior with conditional rules.

Depends on Phase 4.

### Phase 6: Migrate built-in tool definitions

- Update `.jp/mcp/tools/fs/*.toml` to use `type = "path"` for path parameters.
- Update `.jp/mcp/tools/git/*.toml` similarly.
- Optionally add example `policy.run` configurations to tool documentation.

Depends on Phase 1.
Can proceed in parallel with Phases 2 to 5.

## References

- [RFD 076]: Tool access grants.
  Defines `AccessPolicy` types and cooperative enforcement, and shares the
  `path` parameter type and path-prefix matching logic.
- [RFD 075]: Tool sandbox and access policy.
  Enforces access grants at the OS level, complementing this RFD's prompt-level
  control.
- [RFD 042]: Tool options.
  Establishes per-tool `options` configuration.
- [RFC 6901]: JSON Pointer.
  Defines the syntax used for `arg` paths.
- [JSON Schema Validation]: Validation vocabulary.
  Defines the `const`, `enum`, `pattern`, `minimum`, `maximum`,
  `exclusive_minimum`, and `exclusive_maximum` keywords reused here.
- [`jsonptr` crate]: Rust implementation of RFC 6901 used for pointer parsing.

[JSON Schema Validation]: https://www.ietf.org/archive/id/draft-bhutton-json-schema-validation-01.html
[RFC 6901]: https://www.rfc-editor.org/rfc/rfc6901.html
[RFD 042]: ../042-tool-options.md
[RFD 043]: ../043-incremental-tool-call-argument-streaming.md
[RFD 075]: ../075-tool-sandbox-and-access-policy.md
[RFD 076]: ../076-tool-access-grants.md
[RFD D26]: D26-streaming-policy-evaluation-for-tool-call-arguments.md
[`jsonptr` crate]: https://crates.io/crates/jsonptr
[json-schema-validation]: https://www.ietf.org/archive/id/draft-bhutton-json-schema-validation-01.html
[jsonptr]: https://crates.io/crates/jsonptr
[rfc-6901]: https://www.rfc-editor.org/rfc/rfc6901.html
